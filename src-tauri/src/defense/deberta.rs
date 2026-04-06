//! DeBERTa v3 ONNX-based prompt injection detector
//!
//! Uses a fine-tuned DeBERTa v3 model (e.g., protectai/deberta-v3-base-prompt-injection-v2)
//! for fast (<10ms) prompt injection detection via ONNX Runtime.

use anyhow::{Context, Result};
use ndarray::Array2;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use tracing::{info, warn};

use crate::guardrails::SecurityLevel;

const DEBERTA_MODEL_URL: &str = "https://huggingface.co/protectai/deberta-v3-base-prompt-injection-v2/resolve/main/onnx/model.onnx";
const DEBERTA_TOKENIZER_URL: &str = "https://huggingface.co/protectai/deberta-v3-base-prompt-injection-v2/resolve/main/tokenizer.json";
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;
const DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Expected SHA-256 of protectai/deberta-v3-base-prompt-injection-v2 onnx/model.onnx.
///
/// **SECURITY**: Downloads are BLOCKED until this is replaced with a real digest.
/// To generate: download model.onnx from HuggingFace, then run:
///   shasum -a 256 model.onnx
/// Paste the 64-character hex string here.
const DEBERTA_MODEL_SHA256: &str =
    "f0ea7f239f765aedbde7c9e163a7cb38a79c5b8853d3f76db5152172047b228c";
/// Expected SHA-256 of protectai/deberta-v3-base-prompt-injection-v2 tokenizer.json.
///
/// **SECURITY**: Downloads are BLOCKED until this is replaced with a real digest.
/// To generate: download tokenizer.json from HuggingFace, then run:
///   shasum -a 256 tokenizer.json
/// Paste the 64-character hex string here.
const DEBERTA_TOKENIZER_SHA256: &str =
    "f0a66ad0d735d8dca9ecac4ff50fcdef4bb6adbadd2941a926844844d2c2059b";

/// DeBERTa-based prompt injection detector
pub struct DeBERTaDetector {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    threshold: f32,
    max_length: usize,
    error_count: AtomicU32,
    security_level: SecurityLevel,
}

impl DeBERTaDetector {
    /// Update the detection threshold without reloading the model.
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
    }

    /// Create a new DeBERTa detector
    ///
    /// `model_path` should point to a directory containing:
    /// - `model.onnx` — the ONNX model
    /// - `tokenizer.json` — the HuggingFace tokenizer
    pub fn new(
        model_path: Option<&str>,
        threshold: f32,
        security_level: SecurityLevel,
    ) -> Result<Self> {
        let base_path = if let Some(p) = model_path {
            let pb = std::path::PathBuf::from(p);
            // Reject path traversal in config-supplied model paths.
            if pb.components().any(|c| c == std::path::Component::ParentDir) {
                anyhow::bail!("Invalid model_path '{}': parent-directory traversal not allowed", p);
            }
            pb
        } else {
            let data_dir = directories::ProjectDirs::from("ai", "nexibot", "desktop")
                .ok_or_else(|| anyhow::anyhow!("Failed to get project directories"))?
                .data_dir()
                .to_path_buf();
            data_dir.join("models").join("deberta-prompt-injection")
        };

        let model_file = base_path.join("model.onnx");
        let tokenizer_file = base_path.join("tokenizer.json");

        if !model_file.exists() {
            info!("[DEBERTA] Model not found, downloading from HuggingFace...");
            std::fs::create_dir_all(&base_path)?;
            Self::download_file(DEBERTA_MODEL_URL, &model_file, DEBERTA_MODEL_SHA256)
                .context("Failed to auto-download DeBERTa model")?;
        }

        if !tokenizer_file.exists() {
            info!("[DEBERTA] Tokenizer not found, downloading from HuggingFace...");
            std::fs::create_dir_all(&base_path)?;
            Self::download_file(DEBERTA_TOKENIZER_URL, &tokenizer_file, DEBERTA_TOKENIZER_SHA256)
                .context("Failed to auto-download DeBERTa tokenizer")?;
        }

        info!("[DEBERTA] Loading ONNX model from {:?}", model_file);
        let session = ort::session::Session::builder()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)?
            .commit_from_file(&model_file)
            .context("Failed to load DeBERTa ONNX model")?;

        info!("[DEBERTA] Loading tokenizer from {:?}", tokenizer_file);
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_file)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        info!(
            "[DEBERTA] Model loaded successfully (threshold: {})",
            threshold
        );

        Ok(Self {
            session,
            tokenizer,
            threshold,
            max_length: 512,
            error_count: AtomicU32::new(0),
            security_level,
        })
    }

    /// Check if the detector is healthy (error count below threshold)
    pub fn is_healthy(&self) -> bool {
        self.error_count.load(Ordering::Relaxed) < 5
    }

    /// Detect prompt injection in text
    /// Returns (is_injection, confidence)
    /// Fails closed (returns injection=true) in Standard or higher security levels
    pub fn detect(&mut self, text: &str) -> (bool, f32) {
        match self.detect_inner(text) {
            Ok(result) => {
                // Reset error count on success
                self.error_count.store(0, Ordering::Relaxed);
                result
            }
            Err(e) => {
                let count = self.error_count.fetch_add(1, Ordering::Relaxed) + 1;
                warn!("[DEBERTA] Detection failed (error #{}/5): {}", count, e);
                // Fail-closed in Standard or higher security
                match self.security_level {
                    SecurityLevel::Relaxed | SecurityLevel::Disabled => {
                        warn!(
                            "[DEBERTA] Failing open due to {:?} security level",
                            self.security_level
                        );
                        (false, 0.0)
                    }
                    _ => {
                        warn!(
                            "[DEBERTA] Failing closed due to {:?} security level",
                            self.security_level
                        );
                        (true, 1.0)
                    }
                }
            }
        }
    }

    /// Overlap size (in tokens) for windowed scanning of long messages.
    const WINDOW_OVERLAP: usize = 128;

    fn detect_inner(&mut self, text: &str) -> Result<(bool, f32)> {
        // Tokenize
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let ids = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();
        let total_tokens = ids.len();

        // If within max_length, scan normally (no windowing needed)
        if total_tokens <= self.max_length {
            return self.scan_window(ids, attention_mask);
        }

        // Message exceeds 512 tokens: use overlapping window scanning
        let stride = self.max_length - Self::WINDOW_OVERLAP;
        let mut windows: Vec<(usize, usize)> = Vec::new();
        let mut start = 0;
        while start < total_tokens {
            let end = (start + self.max_length).min(total_tokens);
            windows.push((start, end));
            if end == total_tokens {
                break;
            }
            start += stride;
        }

        let num_windows = windows.len();
        warn!(
            "[DEBERTA] Message exceeds 512 tokens ({} tokens), using windowed scanning ({} windows)",
            total_tokens, num_windows
        );

        // Scan each window; if ANY window detects injection, report the highest confidence
        let mut highest_confidence: f32 = 0.0;
        let mut any_injection = false;

        for (win_start, win_end) in &windows {
            let window_ids = &ids[*win_start..*win_end];
            let window_mask = &attention_mask[*win_start..*win_end];
            let (is_injection, confidence) = self.scan_window(window_ids, window_mask)?;
            if is_injection {
                any_injection = true;
            }
            if confidence > highest_confidence {
                highest_confidence = confidence;
            }
        }

        Ok((any_injection, highest_confidence))
    }

    /// Run inference on a single token window and return (is_injection, confidence).
    fn scan_window(&mut self, ids: &[u32], attention_mask: &[u32]) -> Result<(bool, f32)> {
        let len = ids.len().min(self.max_length);
        let input_ids: Vec<i64> = ids[..len].iter().map(|&id| id as i64).collect();
        let attention: Vec<i64> = attention_mask[..len].iter().map(|&m| m as i64).collect();

        // Create tensors
        let input_ids_array = Array2::from_shape_vec((1, len), input_ids)?;
        let attention_mask_array = Array2::from_shape_vec((1, len), attention)?;

        // Build input values using the ort 2.0 API
        let input_ids_value = ort::value::Value::from_array(input_ids_array)
            .context("Failed to create input_ids tensor")?;
        let attention_mask_value = ort::value::Value::from_array(attention_mask_array)
            .context("Failed to create attention_mask tensor")?;

        // Run inference
        let outputs = self.session.run(ort::inputs![
            "input_ids" => input_ids_value,
            "attention_mask" => attention_mask_value,
        ])?;

        // Extract logits and apply softmax
        // ort 2.0.0-rc.10: ValueRef derefs to Value, use try_extract_tensor
        let first_output = outputs.values().next().context("No output tensor found")?;
        let (_shape, raw_logits) = first_output
            .try_extract_tensor::<f32>()
            .context("Failed to extract logits")?;

        // Binary classification: [safe, injection]
        if raw_logits.len() < 2 {
            anyhow::bail!("Expected at least 2 logits, got {}", raw_logits.len());
        }

        // Softmax
        let max_val = raw_logits[0].max(raw_logits[1]);
        let exp0 = (raw_logits[0] - max_val).exp();
        let exp1 = (raw_logits[1] - max_val).exp();
        let sum = exp0 + exp1;
        let injection_prob = exp1 / sum;

        let is_injection = injection_prob >= self.threshold;

        Ok((is_injection, injection_prob))
    }

    fn download_file(url: &str, dest: &std::path::PathBuf, expected_sha256: &str) -> Result<()> {
        info!("[DEBERTA] Downloading from: {}", url);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(
                DOWNLOAD_CONNECT_TIMEOUT_SECS,
            ))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client.get(url).send().context("Failed to download file")?;

        if !response.status().is_success() {
            anyhow::bail!("Download failed with HTTP status: {}", response.status());
        }

        let bytes = response.bytes()?;

        // Verify SHA-256 before writing to disk — guards against supply-chain
        // attacks where HuggingFace CDN delivers a malicious model file.
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
            info!("[DEBERTA] SHA-256 verified for {}", dest.display());
        } else {
            anyhow::bail!(
                "BLOCKED: SHA-256 digest not configured for {}. Model downloads are \
                 disabled until a verified digest is set. To fix: (1) download the \
                 file from HuggingFace (protectai/deberta-v3-base-prompt-injection-v2), \
                 (2) run `shasum -a 256 <file>`, (3) replace the TODO_REPLACE_WITH_SHA256_* \
                 constant in defense/deberta.rs with the 64-char hex digest.",
                dest.display()
            );
        }

        // Atomic write: temp file then rename
        let temp_path = dest.with_extension("tmp");
        std::fs::write(&temp_path, &bytes)?;
        std::fs::rename(&temp_path, dest)?;

        info!(
            "[DEBERTA] Downloaded {} ({} bytes)",
            dest.display(),
            bytes.len()
        );
        Ok(())
    }
}
