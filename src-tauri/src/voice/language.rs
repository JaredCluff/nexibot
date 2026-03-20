//! Language detection and Piper voice model selection for multilingual TTS/STT.
//!
//! After each STT transcription, the pipeline calls `LanguageManager::update()`
//! with the transcript text.  The manager detects the dominant script/language
//! from Unicode codepoint ranges and, if the language has changed, returns the
//! path to the matching Piper voice model so the caller can hot-swap it.
//!
//! Supported auto-detection:
//!   • CJK Unified Ideographs       → "zh" (Mandarin / Chinese)
//!   • Hiragana / Katakana          → "ja" (Japanese)
//!   • Hangul                       → "ko" (Korean)
//!   • Cyrillic                     → "ru" (Russian)
//!   • Arabic                       → "ar" (Arabic)
//!   • Hebrew                       → "he" (Hebrew)
//!   • Devanagari                   → "hi" (Hindi)
//!   • Thai                         → "th" (Thai)
//!   • Greek                        → "el" (Greek)
//!   • Otherwise                    → "en" (English / Latin-script fallback)
//!
//! Piper model filenames follow the official naming convention:
//!   `{lang}_{region}-{voice_name}-{quality}.onnx`
//! e.g. `en_US-lessac-medium.onnx`, `de_DE-thorsten-medium.onnx`
//!
//! The model directory defaults to `<data_dir>/models/piper/`.  Users can
//! override it via `config.tts.piper_model_path` (which sets the *current*
//! model path directly and skips auto-selection).

use std::path::PathBuf;
use tracing::{debug, info};

// ── Known Piper voice models (lang-code → preferred filename prefix) ──────────
//
// These are the "medium" quality models from the official piper release.
// The manager scans the model directory for files whose names start with
// the listed prefix, taking the first match.  This avoids hard-coding full
// filenames and tolerates minor version suffixes.

const LANG_VOICE_PREFIXES: &[(&str, &str)] = &[
    ("en", "en_US-lessac-medium"),
    ("de", "de_DE-thorsten-medium"),
    ("fr", "fr_FR-upmc-medium"),
    ("es", "es_ES-davefx-medium"),
    ("it", "it_IT-riccardo-x_low"),
    ("pt", "pt_BR-faber-medium"),
    ("nl", "nl_NL-mls-medium"),
    ("ru", "ru_RU-dmitri-medium"),
    ("zh", "zh_CN-huayan-medium"),
    ("ja", "ja_JP-naist_jdic-medium"),
    ("ko", "ko_KR-iu-medium"),
    ("ar", "ar_JO-kareem-medium"),
    ("hi", "hi_IN-dhruva-medium"),
    ("pl", "pl_PL-mls_8098-medium"),
    ("cs", "cs_CZ-jirka-medium"),
    ("sv", "sv_SE-nst-medium"),
    ("tr", "tr_TR-fahrettin-medium"),
    ("uk", "uk_UA-lada-x_low"),
];

// ── Language detection from Unicode script ranges ─────────────────────────────

/// Detect the dominant language from the script of characters in `text`.
///
/// Returns a BCP-47 language tag (e.g. `"en"`, `"zh"`, `"ru"`) or `None` if
/// the text is too short or entirely ASCII punctuation/digits.
pub fn detect_language_from_text(text: &str) -> Option<String> {
    if text.trim().len() < 3 {
        return None;
    }

    let mut zh = 0usize;
    let mut ja = 0usize;
    let mut ko = 0usize;
    let mut ru = 0usize;
    let mut ar = 0usize;
    let mut he = 0usize;
    let mut hi = 0usize;
    let mut th = 0usize;
    let mut el = 0usize;
    let mut latin = 0usize;

    for ch in text.chars() {
        let cp = ch as u32;
        match cp {
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF => zh += 1,
            0x3040..=0x309F | 0x30A0..=0x30FF => ja += 1,
            0xAC00..=0xD7AF | 0x1100..=0x11FF => ko += 1,
            0x0400..=0x04FF => ru += 1,
            0x0600..=0x06FF | 0x0750..=0x077F => ar += 1,
            0x0590..=0x05FF => he += 1,
            0x0900..=0x097F => hi += 1,
            0x0E00..=0x0E7F => th += 1,
            0x0370..=0x03FF => el += 1,
            0x0041..=0x007A | 0x00C0..=0x024F => latin += 1,
            _ => {}
        }
    }

    // Pick the dominant script
    let scores = [
        ("zh", zh),
        ("ja", ja),
        ("ko", ko),
        ("ru", ru),
        ("ar", ar),
        ("he", he),
        ("hi", hi),
        ("th", th),
        ("el", el),
        ("en", latin),
    ];

    let (best_lang, best_count) = scores
        .iter()
        .max_by_key(|(_, c)| *c)
        .copied()
        .unwrap_or(("en", 0));

    if best_count == 0 {
        return None;
    }

    Some(best_lang.to_string())
}

// ── LanguageManager ──────────────────────────────────────────────────────────

/// Manages detected language state and maps languages to Piper voice models.
pub struct LanguageManager {
    /// Currently active language (BCP-47 tag, e.g. "en", "de", "zh").
    current_language: Option<String>,
    /// User-configured preferred language.  When set, auto-detection is
    /// disabled and this language is always used.
    preferred_language: Option<String>,
    /// Directory containing downloaded Piper `.onnx` model files.
    model_dir: PathBuf,
}

impl LanguageManager {
    /// Create a new `LanguageManager`.
    ///
    /// # Arguments
    /// * `preferred_language` — If `Some`, disables auto-detection and always
    ///   uses this language for voice selection.
    /// * `model_dir` — Directory that contains Piper `.onnx` model files.
    pub fn new(preferred_language: Option<String>, model_dir: PathBuf) -> Self {
        Self {
            current_language: None,
            preferred_language,
            model_dir,
        }
    }

    /// Update the detected language from a new transcript.
    ///
    /// If auto-detection is active (no preferred language), this detects the
    /// language from `transcript` and updates the internal state.
    ///
    /// Returns `Some(new_lang)` if the language changed from the previous
    /// value, `None` otherwise.
    pub fn update_from_transcript(&mut self, transcript: &str) -> Option<String> {
        // If a fixed preferred language is configured, never override it.
        if let Some(lang) = self.preferred_language.clone() {
            if self.current_language.as_deref() != Some(lang.as_str()) {
                self.current_language = Some(lang.clone());
                return Some(lang);
            }
            return None;
        }

        let detected = match detect_language_from_text(transcript) {
            Some(l) => l,
            None => return None,
        };

        if self.current_language.as_deref() == Some(detected.as_str()) {
            // Language unchanged
            return None;
        }

        debug!(
            "[LANGUAGE] Detected language change: {:?} → {}",
            self.current_language, detected
        );
        self.current_language = Some(detected.clone());
        Some(detected)
    }

    /// Select the Piper voice model path for the given language.
    ///
    /// Scans `self.model_dir` for a `.onnx` file whose name starts with the
    /// prefix registered for `lang`.  Falls back to English if no match is
    /// found for the requested language.
    ///
    /// Returns `None` if the model directory does not exist or is empty.
    pub fn select_piper_voice(&self, lang: &str) -> Option<PathBuf> {
        self.find_model(lang)
            .or_else(|| {
                if lang != "en" {
                    debug!(
                        "[LANGUAGE] No Piper model for '{}', falling back to English",
                        lang
                    );
                    self.find_model("en")
                } else {
                    None
                }
            })
    }

    /// Current active language code (`None` until first transcript processed).
    pub fn current_language(&self) -> Option<&str> {
        self.current_language.as_deref()
    }

    /// Default Piper model directory: `<data_dir>/models/piper/`.
    pub fn default_model_dir() -> PathBuf {
        directories::ProjectDirs::from("ai", "nexibot", "desktop")
            .map(|d| d.data_dir().join("models").join("piper"))
            .unwrap_or_else(|| PathBuf::from("models/piper"))
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn find_model(&self, lang: &str) -> Option<PathBuf> {
        let prefix = LANG_VOICE_PREFIXES
            .iter()
            .find(|(l, _)| *l == lang)
            .map(|(_, p)| *p)?;

        if !self.model_dir.exists() {
            return None;
        }

        // Scan directory for a file starting with `prefix` and ending in `.onnx`
        std::fs::read_dir(&self.model_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("onnx")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(prefix))
                        .unwrap_or(false)
            })
            .map(|p| {
                info!("[LANGUAGE] Selected Piper model: {:?}", p);
                p
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_english() {
        assert_eq!(
            detect_language_from_text("Hello, how are you doing today?"),
            Some("en".to_string())
        );
    }

    #[test]
    fn test_detect_chinese() {
        assert_eq!(
            detect_language_from_text("你好，今天天气怎么样？"),
            Some("zh".to_string())
        );
    }

    #[test]
    fn test_detect_japanese() {
        assert_eq!(
            detect_language_from_text("こんにちは、元気ですか？"),
            Some("ja".to_string())
        );
    }

    #[test]
    fn test_detect_russian() {
        assert_eq!(
            detect_language_from_text("Привет, как дела?"),
            Some("ru".to_string())
        );
    }

    #[test]
    fn test_detect_arabic() {
        assert_eq!(
            detect_language_from_text("مرحبا، كيف حالك؟"),
            Some("ar".to_string())
        );
    }

    #[test]
    fn test_too_short() {
        assert_eq!(detect_language_from_text("hi"), None);
    }

    #[test]
    fn test_language_manager_update() {
        let mut mgr = LanguageManager::new(None, PathBuf::from("/tmp"));
        let result = mgr.update_from_transcript("Hello world, this is English text.");
        assert_eq!(result, Some("en".to_string()));
        // Second call with same language → no change
        let result2 = mgr.update_from_transcript("More English words here.");
        assert_eq!(result2, None);
    }

    #[test]
    fn test_preferred_language_overrides_detection() {
        let mut mgr = LanguageManager::new(Some("de".to_string()), PathBuf::from("/tmp"));
        let result = mgr.update_from_transcript("Hello this is English text.");
        // Should return "de" not "en" since preferred is set
        assert_eq!(result, Some("de".to_string()));
    }
}
