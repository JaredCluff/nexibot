//! Voice Module for NexiBot
//!
//! Handles wake word detection, speech-to-text, and text-to-speech.
//! Provides a complete voice interaction pipeline.
//! Cross-platform: works on macOS, Windows, and Linux.

pub mod audio;
pub mod language;
pub mod preprocessing;
pub mod stt;
pub mod tts;
pub mod vad;
pub mod wakeword;

use regex::Regex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::claude::ClaudeClient;
use crate::config::NexiBotConfig;

// ── Compiled regexes for strip_markdown (compiled once, reused) ─────────
use std::sync::LazyLock;

static RE_CODE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```[^\n]*\n.*?```").expect("invariant: literal regex is valid")
});
static RE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([^\]]+)\]\([^)]+\)").expect("invariant: literal regex is valid")
});
static RE_BOLD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*([^*]+)\*\*").expect("invariant: literal regex is valid"));
static RE_ITALIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*([^*]+)\*").expect("invariant: literal regex is valid"));
static RE_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s*").expect("invariant: literal regex is valid"));
static RE_BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[\s]*[-*]\s+").expect("invariant: literal regex is valid"));
static RE_NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\d+\.\s+").expect("invariant: literal regex is valid"));
static RE_HR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^---+\s*$").expect("invariant: literal regex is valid"));
static RE_MULTI_NEWLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n{3,}").expect("invariant: literal regex is valid"));
static RE_MULTI_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" {2,}").expect("invariant: literal regex is valid"));

/// Event payload for voice transcript events emitted to the frontend
#[derive(Clone, serde::Serialize)]
struct VoiceTranscriptEvent {
    role: String,
    content: String,
}

/// Wake word detection event
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Wake word detected with confidence score and recent audio for STT confirmation
    WakeWordDetected(f32, Vec<f32>),
    /// Voice detected (from VAD)
    #[allow(dead_code)]
    VoiceDetected,
    /// Silence detected (end of speech)
    #[allow(dead_code)]
    SilenceDetected,
    /// Force return to wake-word-required idle state (from UI stop button or command)
    ForceStop,
}

/// Voice state machine states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceState {
    /// Idle - listening for wake word
    Idle,
    /// Wake word detected
    WakeDetected,
    /// Listening - recording user speech
    Listening,
    /// Processing - STT in progress
    Processing,
    /// Thinking - Claude generating response
    Thinking,
    /// Speaking - TTS playing response
    Speaking,
    /// Sleeping - timeout reached, not responding to wake word
    Sleeping,
}

/// Strip markdown formatting from text for TTS output.
/// Removes formatting that would be read aloud as literal characters.
fn strip_markdown(text: &str) -> String {
    let mut result = text.to_string();

    // Remove code blocks (``` ... ```) → "(code omitted)"
    result = RE_CODE_BLOCK
        .replace_all(&result, "(code omitted)")
        .to_string();

    // Remove inline backticks
    result = result.replace('`', "");

    // Convert [text](url) → "text"
    result = RE_LINK.replace_all(&result, "$1").to_string();

    // Remove bold **text** → text
    result = RE_BOLD.replace_all(&result, "$1").to_string();

    // Remove italic *text* → text (single asterisks only, bold already removed above)
    result = RE_ITALIC.replace_all(&result, "$1").to_string();

    // Remove headers (# at line start)
    result = RE_HEADER.replace_all(&result, "").to_string();

    // Remove bullet markers (- or * at line start)
    result = RE_BULLET.replace_all(&result, "").to_string();

    // Remove numbered list markers (1. 2. etc)
    result = RE_NUMBERED.replace_all(&result, "").to_string();

    // Remove horizontal rules
    result = RE_HR.replace_all(&result, "").to_string();

    // Collapse multiple newlines into two
    result = RE_MULTI_NEWLINE.replace_all(&result, "\n\n").to_string();

    // Collapse multiple spaces into one
    result = RE_MULTI_SPACE.replace_all(&result, " ").to_string();

    result.trim().to_string()
}

/// Extract the first complete sentence from a text buffer.
/// Returns `Some((sentence, remaining))` if a sentence boundary is found.
fn extract_sentence(buffer: &str) -> Option<(String, String)> {
    let patterns = [". ", "! ", "? ", ".\n", "!\n", "?\n"];

    let mut earliest_pos = None;
    let mut earliest_len = 0;

    for pattern in &patterns {
        if let Some(pos) = buffer.find(pattern) {
            if earliest_pos.map_or(true, |p| pos < p) {
                earliest_pos = Some(pos);
                earliest_len = pattern.len();
            }
        }
    }

    // Also check for paragraph breaks (double newline)
    if let Some(pos) = buffer.find("\n\n") {
        if earliest_pos.map_or(true, |p| pos < p) {
            earliest_pos = Some(pos);
            earliest_len = 2;
        }
    }

    if let Some(pos) = earliest_pos {
        let sentence_end = pos + 1; // Include the punctuation mark
        let sentence = buffer[..sentence_end].trim().to_string();
        let remaining = buffer[pos + earliest_len..].to_string();

        if sentence.is_empty() {
            return None;
        }

        Some((sentence, remaining))
    } else {
        None
    }
}

/// Generate a sine wave tone at the given frequency and duration.
/// Returns f32 samples at 16kHz sample rate.
fn generate_tone(frequency: f32, duration_ms: u32, volume: f32) -> Vec<f32> {
    let sample_rate = 16000u32;
    let num_samples = (sample_rate as f32 * duration_ms as f32 / 1000.0) as usize;
    let mut samples = Vec::with_capacity(num_samples);

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (2.0 * std::f32::consts::PI * frequency * t).sin() * volume;

        // Apply fade-in/fade-out envelope (5ms)
        let fade_samples = (sample_rate as f32 * 0.005) as usize;
        let envelope = if i < fade_samples {
            i as f32 / fade_samples as f32
        } else if i > num_samples - fade_samples {
            (num_samples - i) as f32 / fade_samples as f32
        } else {
            1.0
        };

        samples.push(sample * envelope);
    }
    samples
}

/// Audio feedback tones for voice state transitions.
struct FeedbackTones;

impl FeedbackTones {
    /// Rising two-note chime: C5 -> E5 (wake word detected)
    fn wake_chime() -> Vec<f32> {
        let mut samples = generate_tone(523.25, 80, 0.3);
        samples.extend(generate_tone(659.25, 80, 0.3));
        samples
    }

    /// Soft ping: G5 (started listening)
    fn listen_ping() -> Vec<f32> {
        generate_tone(783.99, 60, 0.2)
    }

    /// Descending error tone: E4 -> C4
    fn error_tone() -> Vec<f32> {
        let mut samples = generate_tone(329.63, 80, 0.3);
        samples.extend(generate_tone(261.63, 120, 0.3));
        samples
    }

    /// Gentle descending tone: E5 -> C5 (going to sleep / ending conversation)
    fn sleep_tone() -> Vec<f32> {
        let mut samples = generate_tone(659.25, 60, 0.2);
        samples.extend(generate_tone(523.25, 100, 0.15));
        samples
    }

    /// Play a tone through the given rodio Sink.
    fn play_tone(sink: &rodio::Sink, samples: Vec<f32>) {
        let buffer = rodio::buffer::SamplesBuffer::new(1, 16000, samples);
        sink.append(buffer);
    }
}

/// Public wrapper for stripping markdown from text for TTS output.
/// Used by PTT commands to clean Claude responses before speaking.
pub fn strip_markdown_for_tts(text: &str) -> String {
    strip_markdown(text)
}

/// Wrapper around rodio audio output to allow Send+Sync.
/// The OutputStream contains a cpal::Stream which is !Send on macOS.
/// We only create/drop this on the main thread and access the sink via Mutex.
struct AudioOutput {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
}

// Safety: AudioOutput is only created on the main thread in start().
// The _stream field is never accessed after creation; only the handle is used
// to create a Sink. The Mutex ensures exclusive access.
unsafe impl Send for AudioOutput {}
unsafe impl Sync for AudioOutput {}

/// Register all TTS backends from config (shared by start, test_tts on-demand init, and reinit).
///
/// # Backend priority order (highest priority first)
///
/// Backends are registered in the order they should be tried during fallback.
/// `TtsManager::synthesize` uses the configured `backend` name as the primary;
/// if that backend fails at runtime, `synthesize_with_fallback` walks this list
/// in registration order until one succeeds.
///
/// Priority chain (enforced by registration order below):
///   1. Platform-native local (macOS `say` / Windows SAPI) — zero latency, no deps
///   2. Local ONNX (Piper)                                  — offline, fast, high quality
///   3. Linux espeak-ng                                      — lightweight Linux fallback
///   4. Cloud: ElevenLabs                                    — high quality, requires API key
///   5. Cloud: Cartesia                                      — high quality, requires API key
///
/// Cloud backends are always registered last so they are only reached when
/// every local option has failed or is unavailable.
fn register_tts_backends(tts: &mut tts::TtsManager, config: &NexiBotConfig) {
    // ── Priority 1: Platform-native local TTS ──────────────────────────────
    #[cfg(target_os = "macos")]
    {
        let macos_tts = tts::MacOsSayTts::new(config.tts.macos_voice.clone(), 200);
        tts.register_backend(Box::new(macos_tts));
    }

    #[cfg(target_os = "windows")]
    {
        let windows_tts = if let Some(ref voice) = config.tts.windows_voice {
            tts::WindowsSapiTts::with_voice(voice.clone())
        } else {
            tts::WindowsSapiTts::new()
        };
        tts.register_backend(Box::new(windows_tts));
    }

    // ── Priority 2: Local ONNX (Piper) ────────────────────────────────────
    let piper_tts = tts::PiperTts::with_config(
        config.tts.piper_model_path.clone(),
        config.tts.piper_voice.clone(),
    );
    tts.register_backend(Box::new(piper_tts));

    // ── Priority 3: Linux espeak-ng (lightweight local fallback) ──────────
    #[cfg(target_os = "linux")]
    {
        let espeak_tts = if let Some(ref voice) = config.tts.espeak_voice {
            tts::EspeakTts::with_voice(voice.clone())
        } else {
            tts::EspeakTts::new()
        };
        tts.register_backend(Box::new(espeak_tts));
    }

    // ── Priority 4-5: Cloud backends (last resort) ────────────────────────
    tts.register_backend(Box::new(tts::ElevenLabsTts::new(
        config.tts.elevenlabs_api_key.clone(),
        "21m00Tcm4TlvDq8ikWAM".to_string(),
        "eleven_flash_v2_5".to_string(),
    )));
    tts.register_backend(Box::new(tts::CartesiaTts::new(
        config.tts.cartesia_api_key.clone(),
        config.tts.cartesia_voice_id.clone(),
        config.tts.cartesia_model.clone(),
        config.tts.cartesia_speed,
    )));
}

/// Register all STT backends from config (shared by start and reinit).
async fn register_stt_backends(stt: &mut stt::SttManager, config: &NexiBotConfig) {
    #[cfg(target_os = "macos")]
    {
        let macos_stt = stt::MacOsSpeechStt::new();
        stt.register_backend(Box::new(macos_stt));
    }

    #[cfg(target_os = "windows")]
    {
        let windows_stt = stt::WindowsSpeechStt::new();
        stt.register_backend(Box::new(windows_stt));
    }

    let sensevoice_stt = if let Some(ref model_path) = config.stt.sensevoice_model_path {
        stt::SenseVoiceStt::with_model_path(model_path.clone())
    } else {
        stt::SenseVoiceStt::new()
    };
    stt.register_backend(Box::new(sensevoice_stt));

    // Build the Deepgram rate limiter using per-config settings + the standard
    // usage-file path so monthly budget survives restarts.
    let deepgram_rate_limiter = {
        let usage_path = dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("nexibot/deepgram_usage.json");
        Some(stt::DeepgramRateLimiter::new(
            config.stt.deepgram_rate_limit.clone(),
            usage_path,
        ))
    };
    stt.register_backend(Box::new(stt::DeepgramStt::new(
        config.stt.deepgram_api_key.clone(),
        deepgram_rate_limiter,
    )));
    stt.register_backend(Box::new(stt::OpenAIStt::new(
        config.stt.openai_api_key.clone(),
    )));
}

/// Voice service that orchestrates the entire voice pipeline
pub struct VoiceService {
    /// Current state
    state: Arc<RwLock<VoiceState>>,
    /// Configuration
    config: Arc<RwLock<NexiBotConfig>>,
    /// Claude client
    claude_client: Arc<RwLock<ClaudeClient>>,
    /// Audio capture
    audio_capture: audio::AudioCapture,
    /// STT manager
    stt_manager: Arc<RwLock<stt::SttManager>>,
    /// TTS manager
    tts_manager: Arc<RwLock<tts::TtsManager>>,
    /// Accumulated audio buffer for STT
    audio_buffer: Arc<RwLock<Vec<f32>>>,
    /// Persistent audio output stream (kept alive for the lifetime of the service)
    audio_stream: Arc<std::sync::Mutex<Option<AudioOutput>>>,
    /// Persistent audio sink for non-blocking, interruptible playback
    audio_sink: Arc<std::sync::Mutex<Option<rodio::Sink>>>,
    /// Voice event channel sender
    event_tx: Option<mpsc::UnboundedSender<VoiceEvent>>,
    /// Processing loop handle
    processing_handle: Option<tokio::task::JoinHandle<()>>,
    /// Audio capture thread handle
    audio_thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Exit signal for graceful audio capture thread shutdown
    audio_exit_signal: Arc<std::sync::atomic::AtomicBool>,
    /// Flag to suppress wake word detection during TTS playback (prevents self-triggering)
    suppress_wakeword: Arc<std::sync::atomic::AtomicBool>,
    /// Runtime toggle: when false, TTS is skipped and response is text-only.
    voice_response_enabled: Arc<std::sync::atomic::AtomicBool>,
    /// Application state for tool execution and memory access during voice pipeline
    app_state: Option<crate::commands::AppState>,
    /// Tauri app handle for emitting events to the frontend
    app_handle: Option<tauri::AppHandle>,
    /// Audio preprocessing configuration for improving STT accuracy
    preprocessing_config: preprocessing::PreprocessingConfig,
    /// Language manager for auto language detection and Piper voice selection
    language_manager: Arc<RwLock<language::LanguageManager>>,
}

impl VoiceService {
    /// Create a new voice service
    pub fn new(
        config: Arc<RwLock<NexiBotConfig>>,
        claude_client: Arc<RwLock<ClaudeClient>>,
    ) -> Self {
        info!("[VOICE] Initializing voice service");

        // Initialize STT and TTS managers
        let stt_manager = Arc::new(RwLock::new(stt::SttManager::new()));
        let tts_manager = Arc::new(RwLock::new(tts::TtsManager::new()));

        // LanguageManager is initialised with defaults here; it will be
        // re-created in start() once the config is known.
        let lang_mgr = language::LanguageManager::new(None, std::path::PathBuf::new());

        Self {
            state: Arc::new(RwLock::new(VoiceState::Idle)),
            config,
            claude_client,
            audio_capture: audio::AudioCapture::new(),
            stt_manager,
            tts_manager,
            audio_buffer: Arc::new(RwLock::new(Vec::new())),
            audio_stream: Arc::new(std::sync::Mutex::new(None)),
            audio_sink: Arc::new(std::sync::Mutex::new(None)),
            event_tx: None,
            processing_handle: None,
            audio_thread_handle: None,
            audio_exit_signal: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            suppress_wakeword: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            voice_response_enabled: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            app_state: None,
            app_handle: None,
            preprocessing_config: preprocessing::PreprocessingConfig::default(),
            language_manager: Arc::new(RwLock::new(lang_mgr)),
        }
    }

    /// Set the application state for tool execution and memory access
    pub fn set_app_state(&mut self, state: crate::commands::AppState) {
        self.app_state = Some(state);
    }

    /// Set the Tauri app handle for emitting events
    pub fn set_app_handle(&mut self, handle: tauri::AppHandle) {
        self.app_handle = Some(handle);
    }

    /// Start the voice service
    pub async fn start(&mut self) -> anyhow::Result<()> {
        info!("[VOICE] Starting voice service");

        // Pre-request microphone permission ONCE before any subsystem initialization.
        // This consolidates what would otherwise be 3-5 separate TCC prompts
        // (audio capture, wake word, VAD, STT, push-to-talk) into a single prompt.
        #[cfg(target_os = "macos")]
        {
            use cpal::traits::HostTrait;
            let host = cpal::default_host();
            if host.default_input_device().is_none() {
                warn!("[VOICE] No input audio device available — microphone permission may be denied");
            } else {
                info!("[VOICE] Microphone access confirmed");
            }
        }

        let config = self.config.read().await;

        // Initialize STT backends (platform-conditional)
        {
            let mut stt = self.stt_manager.write().await;
            register_stt_backends(&mut stt, &config).await;
            stt.set_backend(&config.stt.backend).await?;
            info!(
                "[VOICE] STT initialized with backend: {}",
                config.stt.backend
            );
        }

        // Initialize TTS backends (platform-conditional)
        {
            let mut tts = self.tts_manager.write().await;
            register_tts_backends(&mut tts, &config);
            tts.set_backend(&config.tts.backend)?;
            info!(
                "[VOICE] TTS initialized with backend: {}",
                config.tts.backend
            );
        }

        // Reinitialise LanguageManager with live config values
        {
            let model_dir = language::LanguageManager::default_model_dir();
            let new_mgr = language::LanguageManager::new(
                config.stt.preferred_language.clone(),
                model_dir,
            );
            *self.language_manager.write().await = new_mgr;
            info!(
                "[VOICE] LanguageManager initialised (preferred_lang={:?}, auto_detect={})",
                config.stt.preferred_language, config.tts.auto_language_detection
            );
        }

        // Initialize persistent audio stream and sink for TTS playback
        match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => {
                match rodio::Sink::try_new(&handle) {
                    Ok(sink) => {
                        info!("[VOICE] Persistent audio sink initialized");
                        let audio_output = AudioOutput {
                            _stream: stream,
                            handle,
                        };
                        // keep handle reference for later use if needed
                        let _ = &audio_output.handle;
                        *self.audio_stream.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(audio_output);
                        *self.audio_sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
                    }
                    Err(e) => {
                        error!("[VOICE] Failed to create audio sink: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("[VOICE] Failed to create audio output stream: {}", e);
            }
        }

        // Create event channel
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        self.event_tx = Some(event_tx.clone());

        // Get config values for audio thread
        let vad_enabled = config.vad.enabled;
        let vad_threshold = config.vad.threshold;
        let vad_require_silero = config.vad.require_silero;
        let wakeword_enabled = config.wakeword.enabled;
        let sleep_timeout = config.wakeword.sleep_timeout_seconds;
        let wake_word = config.wakeword.wake_word.clone();
        let wakeword_threshold = config.wakeword.threshold;
        let stt_wakeword_enabled = config.wakeword.stt_wakeword_enabled;
        let voice_response_initial = config.wakeword.voice_response_enabled;

        drop(config);

        self.voice_response_enabled
            .store(voice_response_initial, std::sync::atomic::Ordering::SeqCst);

        // Start audio capture loop in background thread
        if wakeword_enabled {
            let stt_capture = self.audio_capture.clone();
            let suppress_wakeword = self.suppress_wakeword.clone();
            // Reset exit signal before spawning (in case of restart)
            self.audio_exit_signal
                .store(false, std::sync::atomic::Ordering::SeqCst);
            let exit_signal = self.audio_exit_signal.clone();
            let audio_thread = std::thread::spawn(move || {
                info!("[VOICE] Starting audio capture thread");

                let result = audio::run_audio_capture_loop(
                    vad_enabled,
                    vad_threshold,
                    vad_require_silero,
                    wakeword_enabled,
                    sleep_timeout,
                    wake_word,
                    wakeword_threshold,
                    stt_wakeword_enabled,
                    stt_capture,
                    suppress_wakeword,
                    exit_signal,
                    move |score, recent_audio| {
                        // Wake word detected callback (includes recent audio for STT confirmation)
                        if let Err(e) =
                            event_tx.send(VoiceEvent::WakeWordDetected(score, recent_audio))
                        {
                            error!("[VOICE] Failed to send wake word event: {}", e);
                        }
                    },
                );

                if let Err(e) = result {
                    error!("[VOICE] Audio capture loop error: {}", e);
                }
            });

            self.audio_thread_handle = Some(audio_thread);
        }

        // Start voice processing loop in background
        self.start_processing_loop(event_rx).await;

        info!("[VOICE] Voice service started");
        Ok(())
    }

    /// Stop the voice service
    pub async fn stop(&mut self) -> anyhow::Result<()> {
        info!("[VOICE] Stopping voice service");

        // Stop processing loop
        if let Some(handle) = self.processing_handle.take() {
            handle.abort();
        }

        // Signal audio capture thread to exit gracefully
        self.audio_exit_signal
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Join the audio thread (may block briefly while the current loop iteration completes)
        if let Some(handle) = self.audio_thread_handle.take() {
            if let Err(e) = handle.join() {
                warn!("[VOICE] Audio thread join error: {:?}", e);
            }
        }

        self.audio_capture.stop()?;
        info!("[VOICE] → {:?}", VoiceState::Idle);
        *self.state.write().await = VoiceState::Idle;

        info!("[VOICE] Voice service stopped");
        Ok(())
    }

    /// Get current state
    pub async fn get_state(&self) -> VoiceState {
        *self.state.read().await
    }

    /// Check if in sleep mode
    pub async fn is_sleeping(&self) -> bool {
        *self.state.read().await == VoiceState::Sleeping
    }

    /// Wake up from sleep mode
    #[allow(dead_code)]
    pub async fn wake_up(&mut self) {
        let mut state = self.state.write().await;
        if *state == VoiceState::Sleeping {
            *state = VoiceState::Idle;
            info!("[VOICE] Woke up from sleep mode");
        }
    }

    /// Force return to wake-word-idle mode; cancels any running pipeline.
    pub async fn force_stop(&self) -> anyhow::Result<()> {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(VoiceEvent::ForceStop);
        }
        Ok(())
    }

    /// Set voice response (TTS) enabled state at runtime.
    pub fn set_voice_response_enabled(&self, enabled: bool) {
        self.voice_response_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get current voice response (TTS) enabled state.
    pub fn get_voice_response_enabled(&self) -> bool {
        self.voice_response_enabled
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Start the audio processing loop in background
    async fn start_processing_loop(&mut self, mut event_rx: mpsc::UnboundedReceiver<VoiceEvent>) {
        let state = self.state.clone();
        let stt = self.stt_manager.clone();
        let tts = self.tts_manager.clone();
        let mut audio_capture = self.audio_capture.clone();
        let audio_buffer = self.audio_buffer.clone();
        let claude_client = self.claude_client.clone();
        let config = self.config.clone();
        let audio_sink_for_loop = self.audio_sink.clone();
        let audio_sink_for_events = self.audio_sink.clone();
        let suppress_wakeword = self.suppress_wakeword.clone();
        let voice_response_flag = self.voice_response_enabled.clone();
        let app_state = self.app_state.clone();
        let app_handle = self.app_handle.clone();
        let preprocessing_config = self.preprocessing_config.clone();
        let language_manager = self.language_manager.clone();

        let handle = tokio::spawn(async move {
            info!("[VOICE] Processing loop started");

            // Timer for listening timeout
            let mut listening_start: Option<tokio::time::Instant> = None;
            const LISTENING_TIMEOUT: Duration = Duration::from_secs(10);

            // Handle to the running Claude+TTS pipeline task (for cancellation on interrupt)
            let mut pipeline_handle: Option<tokio::task::JoinHandle<Option<String>>> = None;
            // Cancellation flag shared with pipeline sub-tasks (TTS consumer, Claude producer).
            // When set, sub-tasks stop producing/synthesizing new content.
            let pipeline_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));

            // Conversation timeout: after speaking ends, revert to wake-word-required mode
            // AtomicI64 stores Unix timestamp (secs) of pipeline completion; 0 = no active conversation.
            // Atomic storage ensures race-free reads/writes across potential future task boundaries.
            let conversation_end_time = Arc::new(std::sync::atomic::AtomicI64::new(0));
            let mut requires_wake_word = true;

            // VAD for end-of-speech detection
            let mut vad_instance: Option<vad::VoiceActivityDetector> =
                match vad::VoiceActivityDetector::new() {
                    Ok(v) => Some(v),
                    Err(e) => {
                        warn!("[VOICE] Failed to initialize VAD: {}", e);
                        None
                    }
                };
            let mut speech_confirmed = false;

            // 3-second cooldown after going dormant (prevents TTS echo from re-triggering wake word)
            const DORMANT_COOLDOWN: Duration = Duration::from_secs(3);
            let mut dormant_cooldown_start: Option<tokio::time::Instant> = None;

            // Sleep mode: track continuous idle time and enter VoiceState::Sleeping when
            // sleep_timeout_seconds elapses with no activity. ONNX detection keeps running in
            // the audio thread while sleeping; a WakeWordDetected event wakes back to Idle.
            let mut idle_since: Option<tokio::time::Instant> = None;

            loop {
                // Suppress wake word during active processing, listening, and cooldown
                {
                    let current = *state.read().await;
                    let in_cooldown =
                        dormant_cooldown_start.is_some_and(|t| t.elapsed() < DORMANT_COOLDOWN);
                    // Suppress during Processing/Thinking/Listening and dormant cooldown.
                    // During Speaking, allow wake word for voice interrupts.
                    let should_suppress = matches!(
                        current,
                        VoiceState::Thinking | VoiceState::Processing | VoiceState::Listening
                    ) || in_cooldown;
                    suppress_wakeword.store(should_suppress, std::sync::atomic::Ordering::SeqCst);
                    // Clear expired cooldown
                    if !in_cooldown && dormant_cooldown_start.is_some() {
                        dormant_cooldown_start = None;
                    }
                }

                tokio::select! {
                    // Handle voice events from audio thread
                    Some(event) = event_rx.recv() => {
                        match event {
                            VoiceEvent::WakeWordDetected(score, recent_audio) => {
                                let current_state = *state.read().await;
                                match current_state {
                                    VoiceState::Idle => {
                                        // Read AND-logic config flags
                                        let (stt_wakeword_enabled, stt_require_both) = {
                                            let cfg = config.read().await;
                                            (cfg.wakeword.stt_wakeword_enabled, cfg.wakeword.stt_require_both)
                                        };

                                        // AND mode: always require STT confirmation regardless of score
                                        let confirmed = if stt_require_both && stt_wakeword_enabled && score < 0.98 {
                                            if recent_audio.is_empty() {
                                                false
                                            } else {
                                                match stt.read().await.transcribe(&recent_audio).await {
                                                    Ok(t) => {
                                                        let t_lower = t.to_lowercase();
                                                        let wake_phrases = ["hey nexus", "hey nexi", "hey nex", "a nexus", "hey nexis"];
                                                        let found = wake_phrases.iter().any(|p| t_lower.contains(p));
                                                        if found {
                                                            info!("[VOICE] Wake word AND-confirmed: '{}' (score: {:.4})", t, score);
                                                        } else {
                                                            info!("[VOICE] Wake word AND-mode rejected — STT heard '{}' (score: {:.4})", t, score);
                                                        }
                                                        found
                                                    }
                                                    Err(e) => {
                                                        info!("[VOICE] STT AND-confirm failed ({}), rejecting wake event", e);
                                                        false
                                                    }
                                                }
                                            }
                                        } else if score >= 0.98 || recent_audio.is_empty() {
                                            // Two-stage confirmation: skip for very high scores or empty audio
                                            true
                                        } else {
                                            // Existing soft confirmation for borderline scores
                                            match stt.read().await.transcribe(&recent_audio).await {
                                                Ok(transcript) => {
                                                    let t = transcript.to_lowercase();
                                                    let has_wake = t.contains("nexus") || t.contains("nexi") || t.contains("nexis");
                                                    if has_wake {
                                                        info!("[VOICE] Wake word STT confirmed: '{}' (score: {:.4})", transcript, score);
                                                    } else {
                                                        info!("[VOICE] Wake word FALSE POSITIVE rejected — STT heard '{}' (score: {:.4})", transcript, score);
                                                    }
                                                    has_wake
                                                }
                                                Err(e) => {
                                                    info!("[VOICE] Wake word STT confirmation failed ({}), accepting score {:.4}", e, score);
                                                    true // Accept on STT failure to avoid blocking legitimate wake words
                                                }
                                            }
                                        };

                                        if confirmed {
                                            info!("[VOICE] Wake word confirmed (score: {:.4}), transitioning to Listening", score);
                                            idle_since = None; // reset sleep inactivity timer
                                            suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                            *state.write().await = VoiceState::WakeDetected;
                                        }
                                    }
                                    VoiceState::Speaking | VoiceState::Thinking => {
                                        // Voice interrupt: skip STT confirmation (TTS audio would
                                        // contaminate the recording). Accept any score that passed
                                        // the audio threshold.
                                        info!("[VOICE] Voice interrupt during {:?} (score: {:.4})", current_state, score);

                                        // Signal all pipeline sub-tasks to stop (TTS consumer, Claude producer)
                                        pipeline_cancel.store(true, std::sync::atomic::Ordering::SeqCst);

                                        // Cancel the running pipeline task
                                        if let Some(handle) = pipeline_handle.take() {
                                            handle.abort();
                                        }

                                        // Clear audio sink to stop queued playback.
                                        // The currently playing chunk may finish but no new
                                        // audio will be appended (cancel flag stops TTS consumer).
                                        if let Ok(guard) = audio_sink_for_events.lock() {
                                            if let Some(ref sink) = *guard {
                                                sink.clear();
                                            }
                                        }

                                        suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                        *state.write().await = VoiceState::WakeDetected;
                                    }
                                    VoiceState::Sleeping => {
                                        // Wake word detected while sleeping — wake up.
                                        // ONNX detection was running throughout sleep, so this is
                                        // a valid audio-based wake event.
                                        info!(
                                            "[VOICE] Wake word detected while sleeping (score: {:.4}), waking up",
                                            score
                                        );
                                        idle_since = None;
                                        suppress_wakeword.store(
                                            false,
                                            std::sync::atomic::Ordering::SeqCst,
                                        );
                                        *state.write().await = VoiceState::WakeDetected;
                                    }
                                    _ => {}
                                }
                            }
                            VoiceEvent::ForceStop => {
                                info!("[VOICE] Force stop: returning to wake-word mode");
                                pipeline_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                                if let Some(handle) = pipeline_handle.take() {
                                    handle.abort();
                                }
                                if let Ok(guard) = audio_sink_for_events.lock() {
                                    if let Some(ref sink) = *guard {
                                        sink.clear();
                                    }
                                }
                                requires_wake_word = true;
                                conversation_end_time.store(0, std::sync::atomic::Ordering::SeqCst);
                                dormant_cooldown_start = Some(tokio::time::Instant::now());
                                idle_since = None; // restart sleep inactivity timer on force-stop
                                suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                *state.write().await = VoiceState::Idle;
                            }
                            _ => {}
                        }
                    }

                    // Process state machine every 100ms
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        let current_state = *state.read().await;

                        match current_state {
                            VoiceState::Idle => {
                                // Idle state: waiting for wake word.
                                // Track continuous idle time; transition to Sleeping on timeout.
                                let idle_start =
                                    idle_since.get_or_insert_with(tokio::time::Instant::now);
                                let sleep_timeout_secs = {
                                    let cfg = config.read().await;
                                    cfg.wakeword.sleep_timeout_seconds
                                };
                                if sleep_timeout_secs > 0
                                    && idle_start.elapsed()
                                        >= Duration::from_secs(sleep_timeout_secs)
                                {
                                    info!(
                                        "[VOICE] Inactivity timeout ({}s), entering sleep mode",
                                        sleep_timeout_secs
                                    );
                                    idle_since = None;
                                    if let Ok(guard) = audio_sink_for_loop.lock() {
                                        if let Some(ref sink) = *guard {
                                            FeedbackTones::play_tone(
                                                sink,
                                                FeedbackTones::sleep_tone(),
                                            );
                                        }
                                    }
                                    info!("[VOICE] → {:?}", VoiceState::Sleeping);
                                    *state.write().await = VoiceState::Sleeping;
                                }
                            }
                            VoiceState::WakeDetected => {
                                // Play wake chime and transition to listening
                                info!("[VOICE] Transitioning to Listening state");
                                {
                                    let sink_ref = audio_sink_for_loop.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Ok(guard) = sink_ref.lock() {
                                            if let Some(ref sink) = *guard {
                                                FeedbackTones::play_tone(sink, FeedbackTones::wake_chime());
                                            }
                                        }
                                    });
                                }
                                tokio::time::sleep(Duration::from_millis(180)).await;
                                {
                                    let sink_ref = audio_sink_for_loop.clone();
                                    tokio::task::spawn_blocking(move || {
                                        if let Ok(guard) = sink_ref.lock() {
                                            if let Some(ref sink) = *guard {
                                                FeedbackTones::play_tone(sink, FeedbackTones::listen_ping());
                                            }
                                        }
                                    });
                                }
                                // Start forwarding audio from capture thread to STT buffer
                                if let Err(e) = audio_capture.clear_samples() {
                                    warn!("[VOICE] Failed to clear audio samples: {}", e);
                                }
                                if let Err(e) = audio_capture.start() {
                                    warn!("[VOICE] Failed to start audio capture: {}", e);
                                }
                                debug!("[VOICE] Audio capture forwarding enabled (is_recording={})", audio_capture.is_recording());
                                suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                *state.write().await = VoiceState::Listening;
                                audio_buffer.write().await.clear();
                                listening_start = Some(tokio::time::Instant::now());
                                // Reset VAD state for new utterance
                                if let Some(ref mut v) = vad_instance {
                                    v.reset();
                                }
                                speech_confirmed = false;
                            }
                            VoiceState::Listening => {
                                // Check for timeout
                                if let Some(start) = listening_start {
                                    if start.elapsed() > LISTENING_TIMEOUT {
                                        if let Err(e) = audio_capture.stop() {
                                            warn!("[VOICE] Failed to stop audio capture: {}", e);
                                        }
                                        if requires_wake_word {
                                            // Normal mode: tell user we didn't hear anything
                                            info!("[VOICE] Listening timeout, returning to Idle");
                                            if let Ok(guard) = audio_sink_for_loop.lock() {
                                                if let Some(ref sink) = *guard {
                                                    FeedbackTones::play_tone(sink, FeedbackTones::error_tone());
                                                }
                                            }
                                            if let Ok(audio) = tts.read().await.synthesize("I didn't hear anything.").await {
                                                if let Ok(guard) = audio_sink_for_loop.lock() {
                                                    if let Some(ref sink) = *guard {
                                                        let _ = VoiceService::play_audio_bytes(sink, &audio);
                                                    }
                                                }
                                            }
                                        } else {
                                            // Conversation mode: check if overall conversation timer expired
                                            let conv_expired = {
                                                let start_ts = conversation_end_time.load(std::sync::atomic::Ordering::SeqCst);
                                                if start_ts == 0 {
                                                    true // No active conversation timer → treat as expired
                                                } else {
                                                    let cfg = config.read().await;
                                                    let timeout_secs = cfg.wakeword.conversation_timeout_seconds;
                                                    drop(cfg);
                                                    let now_ts = std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .unwrap_or_default()
                                                        .as_secs() as i64;
                                                    timeout_secs > 0 && (now_ts - start_ts) as u64 > timeout_secs
                                                }
                                            };

                                            if conv_expired {
                                                // Conversation over: go dormant with 3s cooldown
                                                info!("[VOICE] Conversation timeout, returning to dormant mode (3s cooldown)");
                                                requires_wake_word = true;
                                                conversation_end_time.store(0, std::sync::atomic::Ordering::SeqCst);
                                                dormant_cooldown_start = Some(tokio::time::Instant::now());
                                                if let Ok(guard) = audio_sink_for_loop.lock() {
                                                    if let Some(ref sink) = *guard {
                                                        FeedbackTones::play_tone(sink, FeedbackTones::sleep_tone());
                                                    }
                                                }
                                            } else {
                                                // Still in conversation window: re-enter Listening silently
                                                info!("[VOICE] Conversation mode: no speech, re-listening");
                                                if let Err(e) = audio_capture.clear_samples() {
                                                    warn!("[VOICE] Failed to clear audio samples: {}", e);
                                                }
                                                if let Err(e) = audio_capture.start() {
                                                    warn!("[VOICE] Failed to start audio capture: {}", e);
                                                }
                                                audio_buffer.write().await.clear();
                                                listening_start = Some(tokio::time::Instant::now());
                                                if let Some(ref mut v) = vad_instance {
                                                    v.reset();
                                                }
                                                speech_confirmed = false;
                                                continue; // Stay in Listening, don't transition to Idle
                                            }
                                        }
                                        info!("[VOICE] → {:?} (listen timeout, no speech)", VoiceState::Idle);
                                        suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                        *state.write().await = VoiceState::Idle;
                                        listening_start = None;
                                        vad_instance = match vad::VoiceActivityDetector::new() {
                                            Ok(v) => Some(v),
                                            Err(e) => {
                                                warn!("[VOICE] Failed to reinitialize VAD: {}", e);
                                                None
                                            }
                                        };
                                        speech_confirmed = false;
                                        continue;
                                    }
                                }

                                // Drain new audio samples from the capture thread
                                if let Ok(samples) = audio_capture.take_samples() {
                                    if !samples.is_empty() {
                                        debug!("[VOICE] Listening: got {} new samples from capture", samples.len());
                                        let mut buffer = audio_buffer.write().await;
                                        buffer.extend(&samples);

                                        // Feed audio to VAD if available
                                        let buffer_samples = buffer.len();
                                        if let Some(ref mut v) = vad_instance {
                                            if let Ok(is_voice) = v.process_chunk(&samples) {
                                                if is_voice {
                                                    speech_confirmed = true;
                                                }
                                                // Trigger STT when speech confirmed and silence detected
                                                if speech_confirmed && v.is_silence_detected() {
                                                    info!("[VOICE] VAD: speech ended after silence, triggering STT ({} samples)", buffer_samples);
                                                    if let Err(e) = audio_capture.stop() {
                                                        warn!("[VOICE] Failed to stop audio capture: {}", e);
                                                    }
                                                    drop(buffer);
                                                    suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                                    *state.write().await = VoiceState::Processing;
                                                    listening_start = None;
                                                    if let Some(ref mut v2) = vad_instance {
                                                        v2.reset();
                                                    }
                                                    speech_confirmed = false;
                                                    continue;
                                                }
                                            }

                                            // Safety net: if VAD detected speech but hasn't detected
                                            // end-of-speech after 10+ seconds, run STT anyway.
                                            // This handles very long utterances without natural pauses.
                                            if speech_confirmed && buffer_samples >= 16000 * 10 {
                                                info!("[VOICE] VAD safety net: 10s audio buffered, triggering STT ({} samples)", buffer_samples);
                                                if let Err(e) = audio_capture.stop() {
                                                    warn!("[VOICE] Failed to stop audio capture: {}", e);
                                                }
                                                drop(buffer);
                                                suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Processing;
                                                listening_start = None;
                                                if let Some(ref mut v2) = vad_instance {
                                                    v2.reset();
                                                }
                                                speech_confirmed = false;
                                                continue;
                                            }
                                        } else {
                                            // No VAD: trigger STT after 3 seconds of audio
                                            if buffer_samples >= 16000 * 3 {
                                                info!("[VOICE] No-VAD fallback: 3s audio captured, triggering STT");
                                                if let Err(e) = audio_capture.stop() {
                                                    warn!("[VOICE] Failed to stop audio capture: {}", e);
                                                }
                                                drop(buffer);
                                                suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Processing;
                                                listening_start = None;
                                                continue;
                                            }
                                        }
                                    }
                                }
                            }
                            VoiceState::Processing => {
                                // STT processing
                                let buffer = {
                                    let buf = audio_buffer.read().await;
                                    buf.clone()
                                };

                                if !buffer.is_empty() {
                                    // Preprocess audio before STT to improve accuracy
                                    let mut preprocessed = buffer;
                                    preprocessing::preprocess_audio(
                                        &mut preprocessed,
                                        &preprocessing_config,
                                        16000, // target sample rate
                                    );
                                    info!("[VOICE] Transcribing {} samples (preprocessed)", preprocessed.len());

                                    // SttManager::transcribe() enforces a 15s internal timeout per
                                    // backend. A second outer timeout here would stack to 30s
                                    // worst-case. Use the single inner timeout only.
                                    match stt.read().await.transcribe(&preprocessed).await {
                                        Ok(transcript) => {
                                            info!("[VOICE] Transcribed: {}", transcript);

                                            // Clear audio buffer
                                            if let Err(e) = audio_capture.clear_samples() {
                                                warn!("[VOICE] Failed to clear audio samples: {}", e);
                                            }
                                            audio_buffer.write().await.clear();

                                            // ── Language auto-detection: swap Piper voice if needed ──
                                            {
                                                let auto_detect = config.read().await.tts.auto_language_detection;
                                                if auto_detect {
                                                    if let Some(new_lang) = language_manager.write().await.update_from_transcript(&transcript) {
                                                        if let Some(model_path) = language_manager.read().await.select_piper_voice(&new_lang) {
                                                            tts.write().await.set_piper_model(model_path);
                                                        }
                                                    }
                                                }
                                            }

                                            // Skip empty or non-speech transcriptions
                                            let trimmed = transcript.trim().to_lowercase();
                                            if trimmed.is_empty() || trimmed == "no speech detected" || trimmed.len() < 2 {
                                                info!("[VOICE] No meaningful speech detected, returning to Idle");
                                                suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Idle;
                                                // Don't reset conversation timer — let it count down naturally
                                                continue;
                                            }

                                            // Strip wake phrase from transcription.
                                            // STT may capture "hey nexus" as part of the transcript when the user
                                            // speaks the wake word and their question in one breath.
                                            let wake_prefixes = [
                                                "hey nexus ", "hey nexus, ", "hey nexus. ",
                                                "hey nexi ", "hey nexi, ", "hey nexi. ",
                                                "hey nexus", "hey nexi",
                                            ];
                                            let mut transcript = transcript.trim().to_string();
                                            let transcript_lower = transcript.to_lowercase();
                                            for prefix in &wake_prefixes {
                                                if transcript_lower.starts_with(prefix) {
                                                    transcript = transcript[prefix.len()..].trim().to_string();
                                                    info!("[VOICE] Stripped wake phrase, remaining: '{}'", transcript);
                                                    break;
                                                }
                                            }

                                            // If transcript was ONLY the wake phrase (nothing else),
                                            // stay in Listening to wait for the actual question
                                            if transcript.trim().is_empty() {
                                                info!("[VOICE] Only wake phrase detected, continuing to listen");
                                                if let Err(e) = audio_capture.clear_samples() {
                                                    warn!("[VOICE] Failed to clear audio samples: {}", e);
                                                }
                                                if let Err(e) = audio_capture.start() {
                                                    warn!("[VOICE] Failed to start audio capture: {}", e);
                                                }
                                                audio_buffer.write().await.clear();
                                                suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Listening;
                                                listening_start = Some(tokio::time::Instant::now());
                                                if let Some(ref mut v) = vad_instance {
                                                    v.reset();
                                                }
                                                speech_confirmed = false;
                                                continue;
                                            }

                                            // Re-check trimmed after wake phrase stripping
                                            let trimmed = transcript.trim().to_lowercase();

                                            // Check for conversation-ending phrases
                                            let goodbye_phrases = [
                                                "goodbye", "good bye", "bye bye", "bye",
                                                "that's all", "thats all", "that is all",
                                                "go to sleep", "stop listening", "never mind",
                                                "nevermind", "i'm done", "im done",
                                                "stop stop stop", "stop talking",
                                            ];
                                            if goodbye_phrases.iter().any(|p| trimmed == *p || trimmed.starts_with(&format!("{} ", p))) {
                                                info!("[VOICE] Goodbye phrase detected ('{}'), returning to dormant (3s cooldown)", trimmed);
                                                requires_wake_word = true;
                                                conversation_end_time.store(0, std::sync::atomic::Ordering::SeqCst);
                                                dormant_cooldown_start = Some(tokio::time::Instant::now());
                                                // Play goodbye tone
                                                if let Ok(guard) = audio_sink_for_loop.lock() {
                                                    if let Some(ref sink) = *guard {
                                                        FeedbackTones::play_tone(sink, FeedbackTones::sleep_tone());
                                                    }
                                                }
                                                suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Idle;
                                                continue;
                                            }

                                            // Streaming TTS pipeline — spawned as background task so the
                                            // main event loop stays responsive for voice interrupts.
                                            suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                            info!("[VOICE] → {:?}", VoiceState::Thinking);
                                            *state.write().await = VoiceState::Thinking;
                                            info!("[VOICE] Processing: '{}'", transcript);

                                            // Play a brief thinking tone so the user knows we heard them
                                            if let Ok(guard) = audio_sink_for_loop.lock() {
                                                if let Some(ref sink) = *guard {
                                                    let mut tone = generate_tone(523.25, 50, 0.15);
                                                    tone.extend(generate_tone(659.25, 50, 0.15));
                                                    tone.extend(generate_tone(783.99, 70, 0.12));
                                                    FeedbackTones::play_tone(sink, tone);
                                                }
                                            }

                                            // Emit user transcript to frontend chat window
                                            if let Some(ref handle) = app_handle {
                                                use tauri::Emitter;
                                                if let Err(e) = handle.emit(
                                                    "voice:transcript",
                                                    VoiceTranscriptEvent {
                                                        role: "user".to_string(),
                                                        content: transcript.clone(),
                                                    },
                                                ) {
                                                    warn!(
                                                        "[VOICE] Failed to emit user voice:transcript: {}",
                                                        e
                                                    );
                                                }
                                            }

                                            // Save user message to memory
                                            if let Some(ref astate) = app_state {
                                                let mut memory = astate.memory_manager.write().await;
                                                if memory.get_current_session().is_none() {
                                                    let _ = memory.start_session();
                                                }
                                                let _ = memory.add_message("user".to_string(), transcript.clone());
                                            }

                                            // Spawn the Claude→TTS pipeline as a cancellable task
                                            pipeline_cancel.store(false, std::sync::atomic::Ordering::SeqCst);
                                            let pipeline_state = state.clone();
                                            let pipeline_tts = tts.clone();
                                            let pipeline_sink = audio_sink_for_loop.clone();
                                            let pipeline_claude = claude_client.clone();
                                            let cancel_flag = pipeline_cancel.clone();
                                            let app_handle_pipeline = app_handle.clone();
                                            let app_state_pipeline = app_state.clone();
                                            let tts_enabled = voice_response_flag.load(std::sync::atomic::Ordering::SeqCst);
                                            let pipeline_task = tokio::spawn(async move {
                                                let (text_tx, mut text_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

                                                // TTS consumer sub-task (conditionally active based on voice_response_enabled)
                                                let tts_clone = pipeline_tts.clone();
                                                let sink_clone = pipeline_sink.clone();
                                                let state_clone = pipeline_state.clone();
                                                let tts_cancel = cancel_flag.clone();
                                                let tts_handle = if tts_enabled { tokio::spawn(async move {
                                                    let mut text_buffer = String::new();
                                                    let mut first_sentence = true;

                                                    while let Some(chunk) = text_rx.recv().await {
                                                        // Check cancellation before processing
                                                        if tts_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                                            info!("[VOICE] TTS consumer cancelled");
                                                            break;
                                                        }

                                                        text_buffer.push_str(&chunk);

                                                        while let Some((sentence, remaining)) = extract_sentence(&text_buffer) {
                                                            text_buffer = remaining;
                                                            let clean = strip_markdown(&sentence);
                                                            if clean.is_empty() { continue; }

                                                            // Check cancellation before each synthesis
                                                            if tts_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                                                info!("[VOICE] TTS synthesis cancelled");
                                                                return;
                                                            }

                                                            if first_sentence {
                                                                info!("[VOICE] → {:?}", VoiceState::Speaking);
                                                                *state_clone.write().await = VoiceState::Speaking;
                                                                first_sentence = false;
                                                            }

                                                            match tokio::time::timeout(
                                                                Duration::from_secs(30),
                                                                tts_clone.read().await.synthesize(&clean),
                                                            ).await {
                                                                Err(_) => {
                                                                    error!("[VOICE] TTS synthesis timed out after 30s");
                                                                }
                                                                Ok(Ok(audio)) => {
                                                                    // Check cancellation before playing
                                                                    if tts_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                                                        return;
                                                                    }
                                                                    if let Ok(g) = sink_clone.lock() {
                                                                        if let Some(ref s) = *g {
                                                                            let _ = VoiceService::play_audio_bytes(s, &audio);
                                                                        }
                                                                    }
                                                                }
                                                                Ok(Err(e)) => error!("[VOICE] TTS failed: {}", e),
                                                            }
                                                        }
                                                    }

                                                    // Flush remaining buffer (skip if cancelled)
                                                    if tts_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                                                        return;
                                                    }
                                                    let remaining = strip_markdown(&text_buffer);
                                                    if !remaining.is_empty() {
                                                        if first_sentence {
                                                            info!("[VOICE] → {:?} (flush remaining buffer)", VoiceState::Speaking);
                                                            *state_clone.write().await = VoiceState::Speaking;
                                                        }
                                                        match tokio::time::timeout(
                                                            Duration::from_secs(30),
                                                            tts_clone.read().await.synthesize(&remaining),
                                                        ).await {
                                                            Ok(Ok(audio)) => {
                                                                if let Ok(g) = sink_clone.lock() {
                                                                    if let Some(ref s) = *g {
                                                                        let _ = VoiceService::play_audio_bytes(s, &audio);
                                                                    }
                                                                }
                                                            }
                                                            Ok(Err(e)) => error!("[VOICE] TTS flush failed: {}", e),
                                                            Err(_) => error!("[VOICE] TTS flush timed out after 30s"),
                                                        }
                                                    }
                                                }) } else {
                                                    // TTS disabled: drain channel so Claude producer isn't blocked
                                                    tokio::spawn(async move {
                                                        while text_rx.recv().await.is_some() {}
                                                    })
                                                };

                                                // Collect tools for agentic voice (reuse chat tool collection)
                                                let all_tools = if let Some(ref astate) = app_state_pipeline {
                                                    let (tools, _, _, _) = crate::commands::chat::collect_all_tools(astate, Some(&transcript), None).await;
                                                    tools
                                                } else {
                                                    vec![]
                                                };

                                                let overrides = crate::session_overrides::SessionOverrides::default();
                                                let text_tx_clone = text_tx.clone();
                                                let client = pipeline_claude.read().await;

                                                // First call: streaming with tools (voice channel for voice-mode prompt)
                                                let mut result = match client
                                                    .send_message_streaming_with_tools_channel(
                                                        &transcript,
                                                        &all_tools,
                                                        &overrides,
                                                        Some(&crate::channel::ChannelSource::Voice),
                                                        move |chunk| { let _ = text_tx_clone.send(chunk); },
                                                    ).await {
                                                        Ok(r) => r,
                                                        Err(e) => {
                                                            error!("[VOICE] Claude request failed: {}", e);
                                                            drop(text_tx);
                                                            tts_handle.abort();
                                                            if let Ok(g) = pipeline_sink.lock() {
                                                                if let Some(ref s) = *g {
                                                                    FeedbackTones::play_tone(s, FeedbackTones::error_tone());
                                                                }
                                                            }
                                                            if let Ok(audio) = pipeline_tts.read().await.synthesize("I'm having trouble connecting. Try again.").await {
                                                                if let Ok(g) = pipeline_sink.lock() {
                                                                    if let Some(ref s) = *g {
                                                                        let _ = VoiceService::play_audio_bytes(s, &audio);
                                                                    }
                                                                }
                                                            }
                                                            return None;
                                                        }
                                                    };

                                                // Tool-use loop (max 10 iterations)
                                                for _iteration in 0..10 {
                                                    if result.tool_uses.is_empty() || result.stop_reason != "tool_use" { break; }
                                                    if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) { break; }

                                                    for tool_use in &result.tool_uses {
                                                        // Special handling for background tasks
                                                        if tool_use.name == "nexibot_background_task" {
                                                            if let Some(ack) = tool_use.input["spoken_acknowledgment"].as_str() {
                                                                let _ = text_tx.send(ack.to_string());
                                                            }
                                                            // Execute tool (spawns background task)
                                                            if let Some(ref astate) = app_state_pipeline {
                                                                let _ = crate::commands::chat::execute_tool_call(
                                                                    &tool_use.name, &tool_use.id, &tool_use.input, astate, None,
                                                                    "voice-session", "voice-agent", None, None, None,
                                                                ).await;
                                                            }
                                                            drop(text_tx);
                                                            let _ = tts_handle.await;
                                                            // Drain sink
                                                            loop {
                                                                if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) { break; }
                                                                let is_empty = {
                                                                    if let Ok(g) = pipeline_sink.lock() {
                                                                        g.as_ref().is_none_or(|s| s.empty())
                                                                    } else { true }
                                                                };
                                                                if is_empty { break; }
                                                                tokio::time::sleep(Duration::from_millis(100)).await;
                                                            }

                                                            // Emit assistant transcript for the acknowledgment
                                                            if let Some(ref handle) = app_handle_pipeline {
                                                                use tauri::Emitter;
                                                                let ack_text = tool_use.input["spoken_acknowledgment"].as_str().unwrap_or("Working on it.");
                                                                if let Err(e) = handle.emit(
                                                                    "voice:transcript",
                                                                    VoiceTranscriptEvent {
                                                                        role: "assistant".to_string(),
                                                                        content: ack_text.to_string(),
                                                                    },
                                                                ) {
                                                                    warn!(
                                                                        "[VOICE] Failed to emit assistant voice:transcript ack: {}",
                                                                        e
                                                                    );
                                                                }
                                                            }
                                                            if let Some(ref astate) = app_state_pipeline {
                                                                let mut memory = astate.memory_manager.write().await;
                                                                let _ = memory.add_message("assistant".to_string(), "Background task delegated.".to_string());
                                                            }
                                                            return Some("Background task delegated".to_string());
                                                        }

                                                        // Emit tool events for chat auditability
                                                        if let Some(ref handle) = app_handle_pipeline {
                                                            use tauri::Emitter;
                                                            if let Err(e) = handle.emit(
                                                                "chat:tool-start",
                                                                serde_json::json!({
                                                                    "name": tool_use.name, "id": tool_use.id,
                                                                }),
                                                            ) {
                                                                warn!(
                                                                    "[VOICE] Failed to emit chat:tool-start for {}: {}",
                                                                    tool_use.name, e
                                                                );
                                                            }
                                                        }

                                                        let tool_result = if let Some(ref astate) = app_state_pipeline {
                                                            crate::commands::chat::execute_tool_call(
                                                                &tool_use.name, &tool_use.id, &tool_use.input, astate, None,
                                                                "voice-session", "voice-agent", None, None, None,
                                                            ).await
                                                        } else {
                                                            "Tool execution unavailable".to_string()
                                                        };

                                                        client.add_tool_result(&tool_use.id, &tool_result).await;

                                                        if let Some(ref handle) = app_handle_pipeline {
                                                            use tauri::Emitter;
                                                            if let Err(e) = handle.emit(
                                                                "chat:tool-result",
                                                                serde_json::json!({
                                                                    "name": tool_use.name, "id": tool_use.id,
                                                                    "success": !tool_result.starts_with("BLOCKED"),
                                                                }),
                                                            ) {
                                                                warn!(
                                                                    "[VOICE] Failed to emit chat:tool-result for {}: {}",
                                                                    tool_use.name, e
                                                                );
                                                            }
                                                        }
                                                    }

                                                    // Trim history after batch completes to avoid orphaned tool blocks
                                                    client.trim_history_if_needed().await;

                                                    // Continue with streaming callback for TTS
                                                    let text_tx_cont = text_tx.clone();
                                                    result = match client.continue_after_tools_streaming(
                                                        &all_tools, &overrides,
                                                        Some(&crate::channel::ChannelSource::Voice),
                                                        move |chunk| { let _ = text_tx_cont.send(chunk); },
                                                    ).await {
                                                        Ok(r) => r,
                                                        Err(e) => {
                                                            error!("[VOICE] Continue after tools failed: {}", e);
                                                            break;
                                                        }
                                                    };
                                                }

                                                let response = result.text.clone();
                                                drop(text_tx); // Signal EOF to TTS consumer

                                                info!("[VOICE] Claude voice response: {} chars", response.len());
                                                let _ = tts_handle.await;

                                                // Wait for sink to finish playing (exit early if cancelled)
                                                loop {
                                                    if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                                                        info!("[VOICE] Pipeline sink drain cancelled");
                                                        break;
                                                    }
                                                    let is_empty = {
                                                        if let Ok(g) = pipeline_sink.lock() {
                                                            g.as_ref().is_none_or(|s| s.empty())
                                                        } else {
                                                            true
                                                        }
                                                    };
                                                    if is_empty { break; }
                                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                                }

                                                // Emit assistant transcript to frontend
                                                if let Some(ref handle) = app_handle_pipeline {
                                                    use tauri::Emitter;
                                                    if let Err(e) = handle.emit(
                                                        "voice:transcript",
                                                        VoiceTranscriptEvent {
                                                            role: "assistant".to_string(),
                                                            content: response.clone(),
                                                        },
                                                    ) {
                                                        warn!(
                                                            "[VOICE] Failed to emit assistant voice:transcript: {}",
                                                            e
                                                        );
                                                    }
                                                }

                                                // Save assistant response to memory
                                                if let Some(ref astate) = app_state_pipeline {
                                                    let mut memory = astate.memory_manager.write().await;
                                                    let _ = memory.add_message("assistant".to_string(), response.clone());
                                                }

                                                if response.is_empty() { None } else { Some(response) }
                                            });

                                            pipeline_handle = Some(pipeline_task);
                                        }
                                        Err(e) => {
                                            error!("[VOICE] STT failed: {}", e);
                                            // Spoken error feedback
                                            if let Ok(guard) = audio_sink_for_loop.lock() {
                                                if let Some(ref sink) = *guard {
                                                    FeedbackTones::play_tone(sink, FeedbackTones::error_tone());
                                                }
                                            }
                                            if let Ok(audio) = tts.read().await.synthesize("Sorry, I didn't catch that.").await {
                                                if let Ok(guard) = audio_sink_for_loop.lock() {
                                                    if let Some(ref sink) = *guard {
                                                        let _ = VoiceService::play_audio_bytes(sink, &audio);
                                                    }
                                                }
                                            }
                                            suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                            *state.write().await = VoiceState::Idle;
                                        }
                                    } // end match stt.transcribe()
                                }
                            }
                            VoiceState::Thinking | VoiceState::Speaking => {
                                // Wait for the pipeline task to complete.
                                // The pipeline itself transitions Thinking→Speaking when the
                                // first TTS sentence arrives, and waits for the sink to drain
                                // before returning. Checking is_finished() avoids premature
                                // transition that sink.empty() would cause between sentences.
                                if let Some(ref handle) = pipeline_handle {
                                    if handle.is_finished() {
                                        let handle = pipeline_handle.take().unwrap();
                                        match handle.await {
                                            Ok(Some(_response)) => {
                                                // Pipeline completed, sink already drained.
                                                // Brief cooldown: TTS audio in the room may still
                                                // be echoing through the mic. Suppress wake word
                                                // for 2s to prevent false positives from TTS echo.
                                                info!("[VOICE] Pipeline finished, 2s cooldown then listening");
                                                dormant_cooldown_start = Some(tokio::time::Instant::now());
                                                requires_wake_word = false;
                                                // Record pipeline-finish timestamp for atomic conversation-timeout checks
                                                let pipeline_finish_ts = std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_secs() as i64;
                                                conversation_end_time.store(pipeline_finish_ts, std::sync::atomic::Ordering::SeqCst);
                                                if let Err(e) = audio_capture.clear_samples() {
                                                    warn!("[VOICE] Failed to clear audio samples: {}", e);
                                                }
                                                if let Err(e) = audio_capture.start() {
                                                    warn!("[VOICE] Failed to start audio capture: {}", e);
                                                }
                                                audio_buffer.write().await.clear();
                                                suppress_wakeword.store(true, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Listening;
                                                listening_start = Some(tokio::time::Instant::now());
                                                if let Some(ref mut v) = vad_instance {
                                                    v.reset();
                                                }
                                                speech_confirmed = false;
                                            }
                                            Ok(None) => {
                                                // Pipeline failed (error already spoken in task)
                                                info!("[VOICE] Pipeline failed, returning to Idle");
                                                dormant_cooldown_start = Some(tokio::time::Instant::now());
                                                suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                                *state.write().await = VoiceState::Idle;
                                            }
                                            Err(e) => {
                                                if e.is_cancelled() {
                                                    // Task was cancelled (voice interrupt)
                                                    // The interrupt handler already changed state
                                                    info!("[VOICE] Pipeline cancelled by interrupt");
                                                } else {
                                                    // Task panicked — recover to Idle so voice isn't stuck
                                                    error!("[VOICE] Pipeline task panicked: {:?}", e);
                                                    dormant_cooldown_start = Some(tokio::time::Instant::now());
                                                    suppress_wakeword.store(false, std::sync::atomic::Ordering::SeqCst);
                                                    *state.write().await = VoiceState::Idle;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            VoiceState::Sleeping => {
                                // Sleep mode: ONNX wake word detection continues running in the
                                // audio thread. STT/LLM processing is skipped while sleeping.
                                // Wake-up is triggered by a WakeWordDetected event (handled in
                                // the event arm of this select! loop).
                            }
                        }
                    }
                }
            }
        });

        self.processing_handle = Some(handle);
    }

    /// Play audio bytes through the persistent sink (non-blocking).
    /// Audio is decoded and appended to the sink queue.
    fn play_audio_bytes(sink: &rodio::Sink, audio: &[u8]) -> anyhow::Result<()> {
        use rodio::Decoder;
        use std::io::Cursor;

        let cursor = Cursor::new(audio.to_vec());
        let source =
            Decoder::new(cursor).map_err(|e| anyhow::anyhow!("Failed to decode audio: {}", e))?;
        sink.append(source);
        Ok(())
    }

    /// Play audio file (blocking, for async context)
    async fn play_audio_blocking(path: &std::path::Path) -> anyhow::Result<()> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || Self::play_audio_file(&path)).await?
    }

    /// Get current STT backend name
    pub async fn get_stt_backend(&self) -> String {
        let config = self.config.read().await;
        config.stt.backend.clone()
    }

    /// Get current TTS backend name
    pub async fn get_tts_backend(&self) -> String {
        let config = self.config.read().await;
        config.tts.backend.clone()
    }

    /// Test TTS by speaking a message
    /// Initializes a TTS backend on-demand if the voice service hasn't been started.
    pub async fn test_tts(
        &self,
        text: &str,
        _voice_override: Option<String>,
    ) -> anyhow::Result<()> {
        info!("[VOICE] Testing TTS with: {}", text);

        let config = self.config.read().await;
        let backend_name = config.tts.backend.clone();

        // Check if backends are registered; if not, initialize on-demand
        let needs_init = {
            let tts = self.tts_manager.read().await;
            tts.get_backend_name().is_empty()
        };

        if needs_init {
            info!("[VOICE] TTS backends not initialized, initializing on-demand for test");
            let mut tts = self.tts_manager.write().await;
            register_tts_backends(&mut tts, &config);
            tts.set_backend(&backend_name)?;
        }
        drop(config);

        let tts = self.tts_manager.read().await;
        let clean_text = strip_markdown_for_tts(text);
        let audio_bytes = tts.synthesize(&clean_text).await?;
        info!("[VOICE] TTS generated {} bytes", audio_bytes.len());

        // Try persistent sink first, fall back to temp file
        let played_via_sink = {
            if let Ok(guard) = self.audio_sink.lock() {
                if let Some(ref sink) = *guard {
                    match Self::play_audio_bytes(sink, &audio_bytes) {
                        Ok(()) => {
                            while !sink.empty() {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                            true
                        }
                        Err(e) => {
                            error!("[VOICE] Persistent sink playback failed: {}", e);
                            false
                        }
                    }
                } else {
                    false
                }
            } else {
                false
            }
        };

        if !played_via_sink {
            let temp_dir = std::env::temp_dir();
            let temp_file = temp_dir.join(format!("nexibot_tts_test_{}.wav", Uuid::new_v4()));
            std::fs::write(&temp_file, &audio_bytes)?;
            Self::play_audio_file(&temp_file)?;
            if temp_file.exists() {
                std::fs::remove_file(&temp_file)?;
            }
        }

        info!("[VOICE] TTS playback completed");
        Ok(())
    }

    /// Reinitialize TTS and STT backends from fresh config (hot-reload).
    /// Takes `&self` — acquires internal write locks on managers, which naturally
    /// blocks until any in-flight synthesis/transcription finishes.
    pub async fn reinit_backends(&self) {
        let config = self.config.read().await;

        // Reinit TTS
        {
            let mut tts = self.tts_manager.write().await;
            tts.clear_backends();
            register_tts_backends(&mut tts, &config);
            match tts.set_backend(&config.tts.backend) {
                Ok(()) => info!("[HOT_RELOAD] TTS backend reloaded: {}", config.tts.backend),
                Err(e) => error!(
                    "[HOT_RELOAD] Failed to set TTS backend '{}': {}",
                    config.tts.backend, e
                ),
            }
        }

        // Reinit STT
        {
            let mut stt = self.stt_manager.write().await;
            stt.clear_backends();
            register_stt_backends(&mut stt, &config).await;
            match stt.set_backend(&config.stt.backend).await {
                Ok(()) => info!("[HOT_RELOAD] STT backend reloaded: {}", config.stt.backend),
                Err(e) => error!(
                    "[HOT_RELOAD] Failed to set STT backend '{}': {}",
                    config.stt.backend, e
                ),
            }
        }
    }

    /// Ensure the STT manager has backends registered and an active backend set.
    /// Handles the case where ptt_transcribe or test_stt is called before start().
    async fn ensure_stt_initialized(&self) -> anyhow::Result<()> {
        let already_set = {
            let stt = self.stt_manager.read().await;
            !stt.get_backend_name().is_empty()
        };

        if !already_set {
            let config = self.config.read().await;
            let mut stt = self.stt_manager.write().await;
            register_stt_backends(&mut stt, &config).await;
            stt.set_backend(&config.stt.backend).await?;
            info!(
                "[VOICE] STT backend lazily initialized: {}",
                config.stt.backend
            );
        }

        Ok(())
    }

    /// Test STT by capturing audio from the microphone and transcribing it
    pub async fn test_stt(&self) -> anyhow::Result<String> {
        info!("[VOICE] Testing STT - recording 3 seconds of audio...");

        self.ensure_stt_initialized().await?;

        let audio_samples = Self::capture_audio_samples(3)?;
        info!("[VOICE] Captured {} audio samples", audio_samples.len());

        if audio_samples.is_empty() {
            anyhow::bail!("No audio captured from microphone");
        }

        let stt = self.stt_manager.read().await;
        let transcript = stt.transcribe(&audio_samples).await?;
        info!("[VOICE] STT test result: {}", transcript);

        Ok(transcript)
    }

    /// Capture audio samples from the default input device
    fn capture_audio_samples(duration_secs: u32) -> anyhow::Result<Vec<f32>> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

        let supported_config = device.default_input_config()?;
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;

        let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
        let samples_clone = Arc::clone(&samples);

        let target_sample_rate = 16000u32;
        let stream = device.build_input_stream(
            &supported_config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut buffer = samples_clone.lock().unwrap_or_else(|e| e.into_inner());
                for chunk in data.chunks(channels) {
                    // Average channels to mono
                    let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    buffer.push(mono);
                }
            },
            |err| {
                tracing::error!("[VOICE] Audio capture error: {}", err);
            },
            None,
        )?;

        stream.play()?;
        std::thread::sleep(std::time::Duration::from_secs(duration_secs as u64));
        drop(stream);

        let captured = samples.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // Resample to 16kHz if needed
        if sample_rate != target_sample_rate {
            let ratio = target_sample_rate as f64 / sample_rate as f64;
            let new_len = (captured.len() as f64 * ratio) as usize;
            let mut resampled = Vec::with_capacity(new_len);
            for i in 0..new_len {
                let src_idx = i as f64 / ratio;
                let idx = src_idx as usize;
                let frac = src_idx - idx as f64;
                let sample = if idx + 1 < captured.len() {
                    captured[idx] * (1.0 - frac as f32) + captured[idx + 1] * frac as f32
                } else if idx < captured.len() {
                    captured[idx]
                } else {
                    0.0
                };
                resampled.push(sample);
            }
            Ok(resampled)
        } else {
            Ok(captured)
        }
    }

    /// Push-to-talk: capture audio samples (blocking for the given duration).
    /// Returns captured f32 samples at 16kHz mono.
    pub fn ptt_capture(duration_secs: u32) -> anyhow::Result<Vec<f32>> {
        Self::capture_audio_samples(duration_secs)
    }

    /// Push-to-talk: transcribe audio samples using the configured STT backend.
    pub async fn ptt_transcribe(&self, samples: &[f32]) -> anyhow::Result<String> {
        self.ensure_stt_initialized().await?;
        let stt = self.stt_manager.read().await;
        stt.transcribe(samples).await
    }

    /// Push-to-talk: send transcribed text to Claude with voice-optimized system prompt.
    pub async fn ptt_send_to_claude(&self, transcript: &str) -> anyhow::Result<String> {
        let claude = self.claude_client.read().await;
        claude
            .send_message_streaming_for_voice(transcript, |_| {})
            .await
    }

    /// Speak text aloud via TTS through the persistent sink.
    pub async fn speak_text(&self, text: &str) -> anyhow::Result<()> {
        let clean = strip_markdown(text);
        let tts = self.tts_manager.read().await;
        let audio_bytes = tts.synthesize(&clean).await?;
        drop(tts);

        // Try to play via persistent sink
        let played = {
            let guard = self.audio_sink.lock();
            if let Ok(ref g) = guard {
                if let Some(ref sink) = **g {
                    let result = Self::play_audio_bytes(sink, &audio_bytes);
                    result.is_ok()
                } else {
                    false
                }
            } else {
                false
            }
            // guard dropped here
        };

        if played {
            // Wait for playback to finish (guard not held across await)
            loop {
                let is_empty = {
                    if let Ok(g) = self.audio_sink.lock() {
                        g.as_ref().is_none_or(|s| s.empty())
                    } else {
                        true
                    }
                };
                if is_empty {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            return Ok(());
        }

        // Fallback: use temp file playback
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("nexibot_tts_{}.wav", Uuid::new_v4()));
        std::fs::write(&temp_file, &audio_bytes)?;
        Self::play_audio_blocking(&temp_file).await?;
        let _ = std::fs::remove_file(&temp_file);
        Ok(())
    }

    /// Synthesize text to audio bytes without playing.
    /// Initializes TTS backend on-demand if not already started.
    /// Returns raw audio bytes (WAV for local backends, MP3 for cloud).
    pub async fn synthesize_text(&self, text: &str) -> anyhow::Result<Vec<u8>> {
        let text = &strip_markdown(text);
        let config = self.config.read().await;
        let backend_name = config.tts.backend.clone();

        // Initialize TTS backends on-demand if needed
        let needs_init = {
            let tts = self.tts_manager.read().await;
            tts.get_backend_name().is_empty()
        };

        if needs_init {
            info!("[VOICE] TTS backends not initialized, initializing on-demand for synthesis");
            let mut tts = self.tts_manager.write().await;
            register_tts_backends(&mut tts, &config);
            let _ = tts.set_backend(&backend_name);
        }
        drop(config);

        let tts = self.tts_manager.read().await;
        let audio_bytes = tts.synthesize(text).await?;
        info!(
            "[VOICE] Synthesized {} bytes for external use",
            audio_bytes.len()
        );
        Ok(audio_bytes)
    }

    /// Play an audio file using rodio
    fn play_audio_file(path: &std::path::Path) -> anyhow::Result<()> {
        use rodio::{Decoder, OutputStream, Sink};
        use std::fs::File;
        use std::io::BufReader;

        // Create output stream
        let (_stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;

        // Load and play audio file
        let file = BufReader::new(File::open(path)?);
        let source = Decoder::new(file)?;
        sink.append(source);

        // Wait for playback to finish
        sink.sleep_until_end();

        Ok(())
    }

    /// Start continuous PTT audio capture in a background thread.
    /// Returns a PttCaptureHandle that can be used to stop and retrieve samples.
    pub fn ptt_start_capture() -> anyhow::Result<PttCaptureHandle> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

        let supported_config = device.default_input_config()?;
        let sample_rate = supported_config.sample_rate().0;
        let channels = supported_config.channels() as usize;

        let samples = Arc::new(std::sync::Mutex::new(Vec::new()));
        let samples_clone = Arc::clone(&samples);
        let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag_clone = Arc::clone(&stop_flag);

        let stream = device.build_input_stream(
            &supported_config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if stop_flag_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let mut buffer = samples_clone.lock().unwrap_or_else(|e| e.into_inner());
                for chunk in data.chunks(channels) {
                    let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                    buffer.push(mono);
                }
            },
            |err| {
                tracing::error!("[PTT] Audio capture error: {}", err);
            },
            None,
        )?;

        stream.play()?;
        info!(
            "[PTT] Capture started (sample_rate: {}, channels: {})",
            sample_rate, channels
        );

        Ok(PttCaptureHandle {
            samples,
            stop_flag,
            _stream: stream,
            sample_rate,
            started_at: std::time::Instant::now(),
        })
    }
}

/// Handle for an active PTT capture session.
/// Keeps the cpal stream alive and collects samples until stopped.
pub struct PttCaptureHandle {
    samples: Arc<std::sync::Mutex<Vec<f32>>>,
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
    _stream: cpal::Stream,
    sample_rate: u32,
    started_at: std::time::Instant,
}

impl PttCaptureHandle {
    /// Stop capture and return the accumulated samples resampled to 16kHz mono.
    pub fn stop(self) -> anyhow::Result<Vec<f32>> {
        self.stop_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let elapsed = self.started_at.elapsed();
        info!("[PTT] Capture stopped after {:.1}s", elapsed.as_secs_f64());

        // Give a moment for the stream callback to finish
        std::thread::sleep(std::time::Duration::from_millis(50));

        let captured = self
            .samples
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let target_sample_rate = 16000u32;

        if self.sample_rate == target_sample_rate {
            return Ok(captured);
        }

        // Resample to 16kHz
        let ratio = target_sample_rate as f64 / self.sample_rate as f64;
        let new_len = (captured.len() as f64 * ratio) as usize;
        let mut resampled = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_idx = i as f64 / ratio;
            let idx = src_idx as usize;
            let frac = src_idx - idx as f64;
            let sample = if idx + 1 < captured.len() {
                captured[idx] * (1.0 - frac as f32) + captured[idx + 1] * frac as f32
            } else if idx < captured.len() {
                captured[idx]
            } else {
                0.0
            };
            resampled.push(sample);
        }
        Ok(resampled)
    }

    /// Get elapsed recording time in seconds.
    #[allow(dead_code)]
    pub fn elapsed_secs(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }
}

// PttCaptureHandle holds a cpal::Stream which is !Send on some platforms,
// but we need it in Tauri state. The stream is only accessed from the thread
// that created it (stop just sets an atomic flag).
unsafe impl Send for PttCaptureHandle {}
unsafe impl Sync for PttCaptureHandle {}
