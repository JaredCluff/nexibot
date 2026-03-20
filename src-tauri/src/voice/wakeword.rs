//! Wake Word Detection Module (adapted from production)
//!
//! Uses OpenWakeWord ONNX models for on-device wake word detection.
//! The detection runs entirely locally - no audio is sent to any server.
//! Models are automatically downloaded on first run if not present.
//!
//! Pipeline: audio → melspectrogram.onnx → embedding_model.onnx → hey_nexus.onnx → score

use ndarray::Array;
use ort::session::{builder::GraphOptimizationLevel, Session};
use sha2::{Digest, Sha256};
use ort::value::Tensor;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Default detection threshold (0.0 - 1.0)
/// Higher = fewer false positives but might miss some activations.
/// 0.85 is the audio-stage threshold — low enough to catch varied pronunciations
/// and mic distances. False positives are filtered by a second-stage STT confirmation
/// in the voice service (checks if transcription actually contains "hey nexus").
const DEFAULT_DETECTION_THRESHOLD: f32 = 0.85;

/// Mel spectrogram parameters (must match OpenWakeWord training)
const N_MELS: usize = 32;
/// Audio samples per processing chunk (80ms at 16kHz)
const CHUNK_SAMPLES: usize = 1280;
/// Extra context samples for streaming mel computation (160 * 3)
const MEL_CONTEXT_SAMPLES: usize = 480;

/// Embedding model parameters
/// Number of mel frames needed for one embedding computation
const EMB_WINDOW_SIZE: usize = 76;
/// Mel frame slide between consecutive embeddings
const EMB_STEP_SIZE: usize = 8;
/// Embedding vector dimensionality (output of embedding model)
const EMB_FEATURES: usize = 96;

/// Wake word model parameters
/// Number of stacked embeddings for wake word classification
const WW_FEATURES: usize = 16;

/// Model download URLs
const OPENWAKEWORD_BASE_URL: &str =
    "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1";

/// Expected SHA-256 digests for OpenWakeWord v0.5.1 pipeline models.
///
/// **SECURITY**: Downloads are BLOCKED until these are replaced with real digests.
/// To generate a digest, download the model manually from a trusted source, then run:
///   shasum -a 256 melspectrogram.onnx
///   shasum -a 256 embedding_model.onnx
///   shasum -a 256 hey_nexus.onnx
/// Paste the 64-character hex string here. This prevents supply-chain attacks
/// where a CDN serves a tampered model file.
const MELSPECTROGRAM_SHA256: &str =
    "ba2b0e0f8b7b875369a2c89cb13360ff53bac436f2895cced9f479fa65eb176f";
const EMBEDDING_MODEL_SHA256: &str =
    "70d164290c1d095d1d4ee149bc5e00543250a7316b59f31d056cff7bd3075c1f";
/// The hey_nexus model is a custom wake word; there is no public download URL.
/// Place the model file manually in the models directory. Update this constant
/// when replacing the model file: `shasum -a 256 hey_nexus.onnx`
const HEY_NEXUS_SHA256: &str = "TODO_REPLACE_WITH_SHA256_OF_HEY_NEXUS_ONNX";

/// Model definitions with their download sources and expected SHA-256 digests
struct ModelInfo {
    filename: String,
    url: String,
    expected_sha256: &'static str,
}

impl ModelInfo {
    fn openwakeword(filename: &str) -> Self {
        let expected_sha256 = match filename {
            "melspectrogram.onnx" => MELSPECTROGRAM_SHA256,
            "embedding_model.onnx" => EMBEDDING_MODEL_SHA256,
            _ => "TODO_REPLACE_WITH_SHA256",
        };
        Self {
            filename: filename.to_string(),
            url: format!("{}/{}", OPENWAKEWORD_BASE_URL, filename),
            expected_sha256,
        }
    }

    fn custom_wakeword(filename: &str) -> Self {
        let expected_sha256 = if filename == "hey_nexus.onnx" {
            HEY_NEXUS_SHA256
        } else {
            "TODO_REPLACE_WITH_SHA256"
        };
        // Custom wake word models must be placed manually in the models directory.
        // There is no default download URL; set one via configuration if needed.
        Self {
            filename: filename.to_string(),
            url: String::new(),
            expected_sha256,
        }
    }
}

/// Wake word detector using OpenWakeWord ONNX models
///
/// Three-stage pipeline:
/// 1. melspectrogram.onnx: audio [1, N] → mel frames [1, 1, F, 32]
/// 2. embedding_model.onnx: mel window [1, 76, 32, 1] → embedding [1, 1, 1, 96]
/// 3. wakeword_model.onnx: stacked embeddings [1, 16, 96] → score
pub struct WakeWordDetector {
    /// ONNX session for mel spectrogram
    mel_session: Session,
    /// ONNX session for wake word embedding
    embedding_session: Session,
    /// ONNX session for wake word classification
    wakeword_session: Session,
    /// Audio context buffer (last MEL_CONTEXT_SAMPLES for streaming overlap)
    audio_context: Vec<f32>,
    /// Audio sample buffer for accumulating incoming samples
    audio_buffer: Vec<f32>,
    /// Rolling buffer of mel spectrogram frames
    mel_buffer: VecDeque<Vec<f32>>,
    /// Rolling buffer of embedding vectors
    embedding_buffer: VecDeque<Vec<f32>>,
    /// Detection threshold (from config)
    threshold: f32,
    /// Last activity timestamp for sleep timeout
    last_activity: Instant,
    /// Sleep timeout duration
    #[allow(dead_code)]
    sleep_timeout: std::time::Duration,
    /// Whether currently in sleep mode
    is_sleeping: AtomicBool,
    /// Whether the detector has been activated at least once (first detection).
    /// Sleep mode only engages after the first detection — not at startup.
    has_been_activated: bool,
}

impl WakeWordDetector {
    /// Create a new wake word detector, downloading models if necessary
    ///
    /// # Arguments
    /// * `sleep_timeout_seconds` - Inactivity timeout before entering sleep mode
    /// * `wake_word` - Wake word phrase (e.g., "hey nexi", "hey nexus")
    /// * `threshold` - Detection threshold (0.0-1.0). If None, uses DEFAULT_DETECTION_THRESHOLD.
    pub fn new(
        sleep_timeout_seconds: u64,
        wake_word: Option<&str>,
        threshold: Option<f32>,
    ) -> anyhow::Result<Self> {
        let threshold = threshold.unwrap_or(DEFAULT_DETECTION_THRESHOLD);
        // Determine model name from wake word
        let model_name = Self::get_model_name(wake_word);

        // Ensure models are available (download if needed)
        let models_dir = Self::ensure_models_available(&model_name)?;

        let mel_path = models_dir.join("melspectrogram.onnx");
        let embedding_path = models_dir.join("embedding_model.onnx");
        let wakeword_path = models_dir.join(&model_name);

        info!("[WAKE] Using models from: {}", models_dir.display());

        // Initialize ONNX sessions
        let mel_session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(&mel_path)?;

        let embedding_session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(&embedding_path)?;

        let wakeword_session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .commit_from_file(&wakeword_path)?;

        info!(
            "[WAKE] Models loaded successfully (threshold: {:.2})",
            threshold
        );

        Ok(Self {
            mel_session,
            embedding_session,
            wakeword_session,
            audio_context: Vec::with_capacity(MEL_CONTEXT_SAMPLES),
            audio_buffer: Vec::with_capacity(CHUNK_SAMPLES * 2),
            mel_buffer: VecDeque::with_capacity(EMB_WINDOW_SIZE + EMB_STEP_SIZE),
            embedding_buffer: VecDeque::with_capacity(WW_FEATURES + 1),
            threshold,
            last_activity: Instant::now(),
            sleep_timeout: std::time::Duration::from_secs(sleep_timeout_seconds),
            is_sleeping: AtomicBool::new(false),
            has_been_activated: false,
        })
    }

    /// Map wake word phrase to model filename
    ///
    /// Maps common wake word phrases to their corresponding ONNX model files.
    /// For "hey nexi" and similar variations, uses "hey_nexus.onnx" as they are
    /// phonetically very similar and the model will recognize both well.
    fn get_model_name(wake_word: Option<&str>) -> String {
        let wake_word = wake_word.unwrap_or("hey nexi").to_lowercase();

        // Map wake words to available models
        match wake_word.as_str() {
            s if s.contains("nexi") || s.contains("nexus") => "hey_nexus.onnx".to_string(),
            s if s.contains("alexa") => "alexa.onnx".to_string(),
            s if s.contains("jarvis") => "hey_jarvis.onnx".to_string(),
            _ => {
                // Default to hey_nexus for unknown wake words
                info!(
                    "[WAKE] Unknown wake word '{}', using hey_nexus model",
                    wake_word
                );
                "hey_nexus.onnx".to_string()
            }
        }
    }

    /// Ensure all required models are available, downloading if necessary
    fn ensure_models_available(wakeword_model: &str) -> anyhow::Result<PathBuf> {
        let models_dir = Self::get_models_dir()?;
        std::fs::create_dir_all(&models_dir)?;

        let required_models = vec![
            ModelInfo::openwakeword("melspectrogram.onnx"),
            ModelInfo::openwakeword("embedding_model.onnx"),
            ModelInfo::custom_wakeword(wakeword_model),
        ];

        for model in required_models {
            let model_path = models_dir.join(&model.filename);

            // Check if already exists locally (including fallback locations)
            if let Some(existing) = Self::find_existing_model(&model.filename) {
                if existing != model_path {
                    info!(
                        "[WAKE] Found {} at {}, copying to app data",
                        model.filename,
                        existing.display()
                    );
                    std::fs::copy(&existing, &model_path)?;
                }
                continue;
            }

            // Download if not found
            info!("[WAKE] Downloading {}...", model.filename);
            Self::download_model(&model.url, &model_path, model.expected_sha256)?;
        }

        Ok(models_dir)
    }

    /// Get the app's models directory
    fn get_models_dir() -> anyhow::Result<PathBuf> {
        // Check environment variable first
        if let Ok(path) = std::env::var("WAKEWORD_MODELS_DIR") {
            return Ok(PathBuf::from(path));
        }

        // Use app data directory (NexiBot-specific)
        if let Some(proj) = directories::ProjectDirs::from("ai", "nexibot", "desktop") {
            return Ok(proj.data_dir().join("models").join("wakeword"));
        }

        // Fallback to home directory
        if let Some(home) = dirs::home_dir() {
            return Ok(home.join(".nexibot").join("models").join("wakeword"));
        }

        Err(anyhow::anyhow!("Could not determine models directory"))
    }

    /// Find an existing model in fallback locations
    fn find_existing_model(filename: &str) -> Option<PathBuf> {
        let search_dirs = Self::get_search_dirs();

        for dir in search_dirs {
            let path = dir.join(filename);
            if path.exists() {
                return Some(path);
            }
        }

        None
    }

    /// Get list of directories to search for existing models.
    ///
    /// Only searches proper app data directories (env override, OS cache, app data).
    /// Never searches user source trees or arbitrary filesystem paths.
    fn get_search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Environment variable override
        if let Ok(path) = std::env::var("WAKEWORD_MODELS_DIR") {
            dirs.push(PathBuf::from(path));
        }

        // OpenWakeWord cache (has melspectrogram.onnx and embedding_model.onnx)
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".cache/openwakeword"));
        }

        // App-specific data directory (NexiBot)
        if let Some(proj) = directories::ProjectDirs::from("ai", "nexibot", "desktop") {
            dirs.push(proj.data_dir().join("models").join("wakeword"));
        }

        dirs
    }

    /// Download a model from URL to the specified path, verifying its SHA-256 digest.
    // TODO: This sync fn uses reqwest::blocking::Client which will block the async runtime
    // if called from an async context (e.g., via WakeWordDetector::new() on a tokio thread).
    // Wrap in tokio::task::spawn_blocking() when WakeWordDetector::new() becomes async,
    // or convert to async reqwest::Client.
    fn download_model(url: &str, dest: &PathBuf, expected_sha256: &str) -> anyhow::Result<()> {
        info!("[WAKE] Downloading from: {}", url);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let response = client.get(url).send()?;

        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download model: HTTP {}",
                response.status()
            ));
        }

        let bytes = response.bytes()?;

        // Verify SHA-256 before writing to disk — guards against supply-chain
        // attacks where the CDN delivers a malicious model file.
        if !expected_sha256.starts_with("TODO") {
            let actual = hex::encode(Sha256::digest(&bytes));
            if actual != expected_sha256 {
                anyhow::bail!(
                    "SHA-256 mismatch for {}: expected {}, got {}. \
                     Refusing to use potentially tampered file.",
                    dest.display(),
                    expected_sha256,
                    actual
                );
            }
            info!("[WAKE] SHA-256 verified for {}", dest.display());
        } else {
            anyhow::bail!(
                "BLOCKED: SHA-256 digest not configured for {}. Model downloads are \
                 disabled until a verified digest is set. To fix: (1) download the \
                 model from a trusted source, (2) run `shasum -a 256 <file>`, \
                 (3) replace the TODO_REPLACE_WITH_SHA256_* constant in \
                 voice/wakeword.rs with the 64-char hex digest.",
                dest.display()
            );
        }

        // Write to temp file first, then rename (atomic)
        let temp_path = dest.with_extension("onnx.tmp");
        std::fs::write(&temp_path, &bytes)?;
        std::fs::rename(&temp_path, dest)?;

        info!(
            "[WAKE] Downloaded {} ({} bytes)",
            dest.display(),
            bytes.len()
        );
        Ok(())
    }

    /// Process audio samples and return detection score if wake word detected.
    ///
    /// Implements the full OpenWakeWord pipeline:
    /// 1. Accumulate 1280-sample audio chunks
    /// 2. Compute mel spectrogram frames (~5 per chunk)
    /// 3. When 76 mel frames accumulated, compute 96-dim embedding
    /// 4. When 16 embeddings accumulated, run wake word classification
    pub fn process_audio(&mut self, samples: &[f32]) -> anyhow::Result<Option<f32>> {
        // Add samples to buffer
        self.audio_buffer.extend_from_slice(samples);

        // Periodic diagnostics
        static CHUNK_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

        // Process complete 1280-sample chunks
        while self.audio_buffer.len() >= CHUNK_SAMPLES {
            let chunk: Vec<f32> = self.audio_buffer.drain(..CHUNK_SAMPLES).collect();
            let count = CHUNK_COUNT.fetch_add(1, Ordering::Relaxed);

            // Build input: prepend context for streaming overlap
            let mut input_samples = self.audio_context.clone();
            input_samples.extend_from_slice(&chunk);

            // Update context: keep last MEL_CONTEXT_SAMPLES from the chunk
            if chunk.len() >= MEL_CONTEXT_SAMPLES {
                self.audio_context = chunk[chunk.len() - MEL_CONTEXT_SAMPLES..].to_vec();
            } else {
                self.audio_context.extend_from_slice(&chunk);
                let excess = self.audio_context.len().saturating_sub(MEL_CONTEXT_SAMPLES);
                if excess > 0 {
                    self.audio_context.drain(..excess);
                }
            }

            // Compute mel spectrogram frames for this chunk
            let mel_frames = match self.compute_mel_frames(&input_samples) {
                Ok(frames) => frames,
                Err(e) => {
                    if count < 3 {
                        warn!("[WAKE] Mel computation error on chunk #{}: {}", count, e);
                    }
                    continue; // Skip this chunk, don't fail the whole pipeline
                }
            };

            if count == 0 {
                info!(
                    "[WAKE] First chunk processed: {} input samples → {} mel frames",
                    input_samples.len(),
                    mel_frames.len()
                );
            }

            // Add new mel frames to buffer
            for frame in mel_frames {
                self.mel_buffer.push_back(frame);
            }
        }

        // Compute embeddings using sliding window over mel frames
        while self.mel_buffer.len() >= EMB_WINDOW_SIZE {
            match self.compute_embedding() {
                Ok(embedding) => {
                    self.embedding_buffer.push_back(embedding);
                    // Keep only the last WW_FEATURES embeddings
                    while self.embedding_buffer.len() > WW_FEATURES {
                        self.embedding_buffer.pop_front();
                    }
                }
                Err(e) => {
                    static EMB_ERR_COUNT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let ec = EMB_ERR_COUNT.fetch_add(1, Ordering::Relaxed);
                    if ec < 3 {
                        warn!("[WAKE] Embedding computation error: {}", e);
                    }
                }
            }
            // Slide mel window forward by EMB_STEP_SIZE frames
            for _ in 0..EMB_STEP_SIZE {
                self.mel_buffer.pop_front();
            }
        }

        // Run wake word detection when we have enough embeddings
        if self.embedding_buffer.len() >= WW_FEATURES {
            let score = self.detect()?;

            // Log notable scores (startup verification, periodic health, meaningful activity)
            static SCORE_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sc = SCORE_COUNT.fetch_add(1, Ordering::Relaxed);
            if sc < 3 || sc.is_multiple_of(500) || score > 0.5 {
                info!(
                    "[WAKE] score #{}: {:.4} (threshold: {:.2})",
                    sc, score, self.threshold
                );
            }

            if score >= self.threshold {
                info!("[WAKE] DETECTED! score={:.4}", score);
                // Clear buffers after detection to avoid re-triggering
                self.mel_buffer.clear();
                self.embedding_buffer.clear();
                self.audio_context.clear();
                self.wake_up();
                return Ok(Some(score));
            }
        }

        Ok(None)
    }

    /// Compute mel spectrogram frames from audio samples.
    ///
    /// Feeds audio to melspectrogram.onnx and extracts normalized mel frames.
    /// Output is normalized as: value / 10.0 + 2.0 (matching OpenWakeWord Python).
    fn compute_mel_frames(&mut self, samples: &[f32]) -> anyhow::Result<Vec<Vec<f32>>> {
        // Input tensor: [1, N_samples]
        let input_array = Array::from_shape_vec((1, samples.len()), samples.to_vec())?;
        let input_tensor = Tensor::from_array(input_array)?;

        // Run mel spectrogram model
        let outputs = self.mel_session.run(ort::inputs![input_tensor])?;

        // Output shape: [1, 1, F, 32] where F = number of mel frames
        let output = outputs[0].try_extract_array::<f32>()?;
        let view = output.view();
        let total_elements = view.len();

        // Extract mel frames dynamically from flattened output
        // Each frame has N_MELS (32) values
        let n_frames = total_elements / N_MELS;
        if n_frames == 0 {
            return Ok(Vec::new());
        }

        let flat: Vec<f32> = view.iter().cloned().collect();
        let mut frames = Vec::with_capacity(n_frames);

        for i in 0..n_frames {
            let start = i * N_MELS;
            let end = start + N_MELS;
            // Normalize to match OpenWakeWord Python: output / 10.0 + 2.0
            let frame: Vec<f32> = flat[start..end].iter().map(|&v| v / 10.0 + 2.0).collect();
            frames.push(frame);
        }

        Ok(frames)
    }

    /// Compute embedding from the current mel frame window.
    ///
    /// Takes the first EMB_WINDOW_SIZE (76) mel frames from the buffer
    /// and feeds them to the embedding model as [1, 76, 32, 1].
    /// Returns a 96-dimensional embedding vector.
    fn compute_embedding(&mut self) -> anyhow::Result<Vec<f32>> {
        // Build input data: [1, 76, 32, 1] = 76 * 32 = 2432 elements
        let mut data = Vec::with_capacity(EMB_WINDOW_SIZE * N_MELS);
        for i in 0..EMB_WINDOW_SIZE {
            let frame = &self.mel_buffer[i];
            if frame.len() >= N_MELS {
                data.extend_from_slice(&frame[..N_MELS]);
            } else {
                data.extend_from_slice(frame);
                data.resize(data.len() + N_MELS - frame.len(), 0.0);
            }
        }

        // Shape: [1, 76, 32, 1]
        let input_array = Array::from_shape_vec((1, EMB_WINDOW_SIZE, N_MELS, 1), data)?;
        let input_tensor = Tensor::from_array(input_array)?;

        // Run embedding model → output [1, 1, 1, 96]
        let outputs = self.embedding_session.run(ort::inputs![input_tensor])?;
        let output = outputs[0].try_extract_array::<f32>()?;
        let embedding: Vec<f32> = output.view().iter().cloned().collect();

        debug!("[WAKE] Embedding computed: {} dims", embedding.len());
        Ok(embedding)
    }

    /// Run wake word classification on accumulated embeddings.
    ///
    /// Stacks the last WW_FEATURES (16) embeddings into [1, 16, 96]
    /// and feeds to the wake word model. Returns detection score (0.0 - 1.0).
    fn detect(&mut self) -> anyhow::Result<f32> {
        // Build input: [1, 16, 96]
        let mut data = Vec::with_capacity(WW_FEATURES * EMB_FEATURES);
        for i in 0..WW_FEATURES {
            let emb = &self.embedding_buffer[i];
            if emb.len() >= EMB_FEATURES {
                data.extend_from_slice(&emb[..EMB_FEATURES]);
            } else {
                data.extend_from_slice(emb);
                data.resize(data.len() + EMB_FEATURES - emb.len(), 0.0);
            }
        }

        let input_array = Array::from_shape_vec((1, WW_FEATURES, EMB_FEATURES), data)?;
        let input_tensor = Tensor::from_array(input_array)?;

        // Run wake word classification
        let outputs = self.wakeword_session.run(ort::inputs![input_tensor])?;
        let scores = outputs[0].try_extract_array::<f32>()?;
        let score = scores.view().iter().cloned().next().unwrap_or(0.0);

        debug!("[WAKE] Detection score: {:.4}", score);
        Ok(score)
    }

    /// Check if sleep timeout has been reached.
    /// Sleep mode only engages after the first successful detection —
    /// at startup the detector stays awake indefinitely waiting for the first wake word.
    #[allow(dead_code)]
    pub fn check_sleep(&self) -> bool {
        if !self.has_been_activated {
            return false;
        }
        if self.last_activity.elapsed() > self.sleep_timeout {
            self.is_sleeping.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }

    /// Wake up from sleep mode
    pub fn wake_up(&mut self) {
        self.has_been_activated = true;
        self.last_activity = Instant::now();
        self.is_sleeping.store(false, Ordering::SeqCst);
    }

    /// Check if currently in sleep mode
    #[allow(dead_code)]
    pub fn is_sleeping(&self) -> bool {
        self.is_sleeping.load(Ordering::SeqCst)
    }

    /// Reset the detector state (keeps activation status)
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.audio_buffer.clear();
        self.audio_context.clear();
        self.mel_buffer.clear();
        self.embedding_buffer.clear();
        self.last_activity = Instant::now();
        self.is_sleeping.store(false, Ordering::SeqCst);
    }
}
