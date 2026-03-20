//! Voice pipeline configurations: Audio, Wake Word, VAD, STT, TTS, and Logging.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_true() -> bool {
    true
}
fn default_conversation_timeout() -> u64 {
    60
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_max_log_file_mb() -> u64 {
    50
}
fn default_max_log_files() -> u32 {
    5
}
fn default_ring_buffer_size() -> usize {
    2000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Enable audio input
    #[serde(default)]
    pub enabled: bool,

    /// Input device name (None = default)
    #[serde(default)]
    pub input_device: Option<String>,

    /// Sample rate
    #[serde(default)]
    pub sample_rate: u32,

    /// Channels (1 = mono, 2 = stereo)
    #[serde(default)]
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakewordConfig {
    /// Enable wake word detection
    pub enabled: bool,

    /// Wake word to detect (e.g., "hey nexus")
    pub wake_word: String,

    /// Detection threshold (0.0-1.0)
    pub threshold: f32,

    /// Path to ONNX model
    pub model_path: Option<PathBuf>,

    /// Sleep timeout in seconds (inactivity before sleep mode)
    pub sleep_timeout_seconds: u64,

    /// Conversation timeout in seconds — after a voice exchange ends,
    /// revert to requiring a wake word if no new interaction within this time.
    /// 0 = never revert (continuous listening). Default: 60.
    #[serde(default = "default_conversation_timeout")]
    pub conversation_timeout_seconds: u64,

    /// STT-based wake word detection enabled (disabled-by-default fallback layer).
    /// Renamed from `stt_fallback` — old configs deserialize via alias.
    #[serde(alias = "stt_fallback", default)]
    pub stt_wakeword_enabled: bool,

    /// When both ONNX and STT wake word are enabled, require BOTH to confirm
    /// before activating (AND logic). Default: false (either triggers = OR).
    #[serde(default)]
    pub stt_require_both: bool,

    /// Synthesize TTS for voice responses. When false, Claude's reply is shown
    /// in the chat window only (no audio playback). Default: true.
    #[serde(default = "default_true")]
    pub voice_response_enabled: bool,

    /// Seconds of idle time after which ONNX wake word models are unloaded from memory.
    /// 0 = never unload (keep loaded as long as voice is active). Default: 0.
    #[serde(default)]
    pub unload_models_after_idle_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VadConfig {
    /// Enable Voice Activity Detection
    pub enabled: bool,

    /// Voice activity threshold (0.0-1.0)
    pub threshold: f32,

    /// Minimum speech duration in milliseconds
    pub min_speech_duration_ms: u64,

    /// Minimum silence duration in milliseconds
    pub min_silence_duration_ms: u64,

    /// If true, fail with an error instead of falling back to RMS energy detection
    /// when the Silero VAD model is unavailable. Defaults to false.
    #[serde(default)]
    pub require_silero: bool,

    /// Push-to-talk mode: VAD only detects the end of speech; the user manually
    /// triggers recording via a button/GPIO. Wake word is not needed in this mode.
    /// Default: false (continuous / always-on VAD).
    #[serde(default)]
    pub push_to_talk: bool,
}

/// Outbound rate-limit config for the Deepgram STT API.
///
/// Defined here (in config) rather than in the voice pipeline to avoid a
/// circular dependency between the config and voice modules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepgramRateLimitConfig {
    /// Enable per-minute call limiting (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum Deepgram STT calls per minute (default: 10)
    #[serde(default = "default_calls_per_minute")]
    pub calls_per_minute: u32,

    /// Monthly audio budget in seconds (default: 720 000 = 200 hrs free tier)
    #[serde(default = "default_monthly_budget_secs")]
    pub monthly_budget_secs: f32,

    /// Block calls when monthly budget is exhausted (default: true).
    /// Set false to allow overage (you will be billed by Deepgram).
    #[serde(default = "default_true")]
    pub block_on_budget_exhausted: bool,
}

fn default_calls_per_minute() -> u32 {
    10
}
fn default_monthly_budget_secs() -> f32 {
    200.0 * 3600.0 // 720 000 s = Deepgram free tier
}

impl Default for DeepgramRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            calls_per_minute: default_calls_per_minute(),
            monthly_budget_secs: default_monthly_budget_secs(),
            block_on_budget_exhausted: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    /// Enable Speech-to-Text
    #[serde(default)]
    pub enabled: bool,

    /// STT backend to use
    #[serde(default)]
    pub backend: String,

    /// Deepgram API key (for cloud STT)
    #[serde(default)]
    pub deepgram_api_key: Option<String>,

    /// OpenAI API key (for cloud STT)
    #[serde(default)]
    pub openai_api_key: Option<String>,

    /// SenseVoice ONNX model path (for local STT)
    #[serde(default)]
    pub sensevoice_model_path: Option<PathBuf>,

    /// Deepgram outbound rate limiter — protects the 200 hrs/month free tier.
    /// Has no effect when the backend is not "deepgram".
    #[serde(default)]
    pub deepgram_rate_limit: DeepgramRateLimitConfig,

    /// Fixed preferred language (BCP-47 tag, e.g. "en", "de", "zh").
    /// When set, auto-language-detection is disabled and TTS always uses
    /// the voice model for this language.  `None` = detect from transcript.
    #[serde(default)]
    pub preferred_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Enable Text-to-Speech
    #[serde(default)]
    pub enabled: bool,

    /// TTS backend to use
    #[serde(default)]
    pub backend: String,

    /// macOS voice name (for macOS say command)
    #[serde(default)]
    pub macos_voice: String,

    /// ElevenLabs API key (for cloud TTS)
    #[serde(default)]
    pub elevenlabs_api_key: Option<String>,

    /// Cartesia API key (for cloud TTS)
    #[serde(default)]
    pub cartesia_api_key: Option<String>,

    /// Cartesia voice ID (default: Barbershop Man)
    #[serde(default)]
    pub cartesia_voice_id: Option<String>,

    /// Cartesia model ID (default: sonic-2024-10-19)
    #[serde(default)]
    pub cartesia_model: Option<String>,

    /// Cartesia speech speed multiplier (0.6–1.5, default: 1.0)
    #[serde(default)]
    pub cartesia_speed: Option<f64>,

    /// Piper ONNX model path (for local TTS)
    #[serde(default)]
    pub piper_model_path: Option<PathBuf>,

    /// Piper voice name
    #[serde(default)]
    pub piper_voice: Option<String>,

    /// espeak-ng voice/language (for Linux TTS fallback)
    #[serde(default)]
    pub espeak_voice: Option<String>,

    /// Windows SAPI voice name
    #[serde(default)]
    pub windows_voice: Option<String>,

    /// Enable automatic language detection from STT transcripts.
    /// When enabled, the Piper voice model is hot-swapped to match the
    /// detected language after each utterance.  Default: false.
    #[serde(default)]
    pub auto_language_detection: bool,
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Master switch for enhanced logging (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum log level: trace, debug, info, warn, error (default: "info")
    #[serde(default = "default_log_level")]
    pub level: String,

    /// Write logs to a file (default: true)
    #[serde(default = "default_true")]
    pub file_enabled: bool,

    /// Log file directory. Default: ~/.config/nexibot/logs/
    #[serde(default)]
    pub file_path: Option<String>,

    /// Maximum size of a single log file in MB before rotation (default: 50)
    #[serde(default = "default_max_log_file_mb")]
    pub max_file_size_mb: u64,

    /// Number of rotated log files to keep (default: 5)
    #[serde(default = "default_max_log_files")]
    pub max_files: u32,

    /// Write logs to stdout/stderr (default: true)
    #[serde(default = "default_true")]
    pub console_enabled: bool,

    /// Automatically redact secrets in log output (default: true)
    #[serde(default = "default_true")]
    pub redact_secrets: bool,

    /// Number of recent log entries to keep in memory for UI streaming (default: 2000)
    #[serde(default = "default_ring_buffer_size")]
    pub ring_buffer_size: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: default_log_level(),
            file_enabled: true,
            file_path: None,
            max_file_size_mb: default_max_log_file_mb(),
            max_files: default_max_log_files(),
            console_enabled: true,
            redact_secrets: true,
            ring_buffer_size: default_ring_buffer_size(),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            input_device: None,
            sample_rate: 16000,
            channels: 1,
        }
    }
}

impl Default for WakewordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            wake_word: "hey nexus".to_string(),
            threshold: 0.85,
            model_path: None,
            sleep_timeout_seconds: 30,
            conversation_timeout_seconds: 60,
            stt_wakeword_enabled: false,
            stt_require_both: false,
            voice_response_enabled: true,
            unload_models_after_idle_secs: 0,
        }
    }
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.5,
            min_speech_duration_ms: 250,
            min_silence_duration_ms: 500,
            require_silero: false,
            push_to_talk: false,
        }
    }
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: if cfg!(target_os = "macos") {
                "macos_speech"
            } else if cfg!(target_os = "windows") {
                "windows_speech"
            } else {
                "sensevoice"
            }
            .to_string(),
            deepgram_api_key: None,
            openai_api_key: None,
            sensevoice_model_path: None,
            deepgram_rate_limit: DeepgramRateLimitConfig::default(),
            preferred_language: None,
        }
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: if cfg!(target_os = "macos") {
                "macos_say"
            } else if cfg!(target_os = "windows") {
                "windows_sapi"
            } else {
                "piper"
            }
            .to_string(),
            macos_voice: "Samantha".to_string(),
            elevenlabs_api_key: None,
            cartesia_api_key: None,
            cartesia_voice_id: None,
            cartesia_model: None,
            cartesia_speed: None,
            piper_model_path: None,
            piper_voice: None,
            espeak_voice: None,
            windows_voice: None,
            auto_language_detection: false,
        }
    }
}
