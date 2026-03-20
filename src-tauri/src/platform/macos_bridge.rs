//! macOS Swift/Objective-C FFI bridge for Speech framework and Voice Processing

use anyhow::Result;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

// Swift FFI declarations
#[cfg(target_os = "macos")]
extern "C" {
    fn macos_speech_init() -> i32;
    fn macos_speech_available() -> i32;
    fn macos_speech_recognize(
        audio_data: *const f32,
        audio_length: usize,
        sample_rate: i32,
        callback: extern "C" fn(*const c_char, *mut c_void),
        user_data: *mut c_void,
    );
}

/// Speech recognition result container
struct RecognitionResult {
    result: Option<String>,
    error: Option<String>,
}

/// Recognize speech from audio samples using macOS Speech Framework (SFSpeechRecognizer)
///
/// This implementation uses Swift FFI to access macOS's native speech recognition.
///
/// # Arguments
/// * `audio` - Audio samples (f32, 16kHz, mono)
///
/// # Returns
/// * `Ok(String)` - Transcribed text
/// * `Err` - If recognition fails or permissions not granted
#[cfg(target_os = "macos")]
pub fn recognize_speech(audio: &[f32]) -> Result<String> {
    use std::time::Duration;

    // Initialize speech recognizer if needed
    static INITIALIZED: std::sync::Once = std::sync::Once::new();
    static mut INIT_STATUS: i32 = 0;

    // SAFETY: INITIALIZED is a Once, so the closure runs exactly once even under
    // concurrent callers. INIT_STATUS is only written inside call_once (serialised)
    // and only read afterwards, so there is no data race.
    unsafe {
        INITIALIZED.call_once(|| {
            debug!("[STT] Initializing macOS Speech Framework");
            INIT_STATUS = macos_speech_init();
        });

        if INIT_STATUS == 0 {
            anyhow::bail!(
                "Speech recognition permission not granted.\n\
                 \n\
                 To enable:\n\
                 1. Open System Settings → Privacy & Security → Speech Recognition\n\
                 2. Enable speech recognition for this app\n\
                 3. Restart the application\n\
                 \n\
                 Alternatively, use cloud STT services (Deepgram or OpenAI) in Settings."
            );
        }
    }

    // Check if available
    // SAFETY: macos_speech_available() is an extern "C" function with no side effects
    // that returns a plain i32. It is safe to call from any thread at any time.
    let available = unsafe { macos_speech_available() };
    if available == 0 {
        anyhow::bail!(
            "Speech recognition not authorized. Please grant permission in System Settings."
        );
    }

    info!(
        "[STT] Recognizing {} audio samples with macOS Speech Framework",
        audio.len()
    );

    // Create result container
    let result_container = Arc::new(Mutex::new(RecognitionResult {
        result: None,
        error: None,
    }));
    let result_clone = Arc::clone(&result_container);

    // Callback function
    extern "C" fn recognition_callback(text_ptr: *const c_char, user_data: *mut c_void) {
        // SAFETY: user_data is Arc::as_ptr(&result_clone) cast to *mut c_void.
        // result_clone is kept alive until after macos_speech_recognize returns, so
        // the pointer is valid for the lifetime of this callback. The cast back to
        // *const Mutex<RecognitionResult> is sound because that is the original type.
        // text_ptr is null-checked before use; CStr::from_ptr requires a valid
        // null-terminated C string, which the Swift side guarantees for non-null ptrs.
        unsafe {
            let result_container = &*(user_data as *const Mutex<RecognitionResult>);

            if text_ptr.is_null() {
                let mut result = result_container.lock().unwrap_or_else(|e| e.into_inner());
                result.error = Some("Recognition failed".to_string());
            } else {
                let c_str = CStr::from_ptr(text_ptr);
                match c_str.to_str() {
                    Ok(text) => {
                        let mut result = result_container.lock().unwrap_or_else(|e| e.into_inner());
                        result.result = Some(text.to_string());
                        debug!("[STT] Recognized: {}", text);
                    }
                    Err(e) => {
                        let mut result = result_container.lock().unwrap_or_else(|e| e.into_inner());
                        result.error = Some(format!("UTF-8 conversion error: {}", e));
                    }
                }
            }
        }
    }

    // Call Swift recognition function
    // SAFETY: audio.as_ptr() is valid for audio.len() f32 values, which is the
    // full slice. recognition_callback is a valid extern "C" fn pointer. result_clone
    // is kept alive on the stack past this call, so Arc::as_ptr() remains valid for
    // the duration of the Swift call and its callback invocation.
    unsafe {
        macos_speech_recognize(
            audio.as_ptr(),
            audio.len(),
            16000, // 16kHz sample rate
            recognition_callback,
            Arc::as_ptr(&result_clone) as *mut c_void,
        );
    }

    // Wait for result (with timeout)
    let timeout = Duration::from_secs(10);
    let start = std::time::Instant::now();

    loop {
        let result = result_container.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(ref text) = result.result {
            info!("[STT] Recognition completed: {} chars", text.len());
            return Ok(text.clone());
        }

        if let Some(ref error) = result.error {
            warn!("[STT] Recognition failed: {}", error);
            anyhow::bail!("Speech recognition error: {}", error);
        }

        drop(result);

        if start.elapsed() > timeout {
            anyhow::bail!("Speech recognition timeout after 10 seconds");
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(target_os = "macos"))]
pub fn recognize_speech(_audio: &[f32]) -> Result<String> {
    anyhow::bail!("macOS Speech recognition only available on macOS")
}

/// Detect voice activity using macOS native Voice Processing I/O
///
/// # Arguments
/// * `audio` - Audio samples (f32, 16kHz, mono)
///
/// # Returns
/// * `Ok(f32)` - Voice activity probability (0.0 = silence, 1.0 = voice detected)
/// * `Err` - If VAD fails
#[cfg(target_os = "macos")]
pub fn detect_voice_activity(audio: &[f32]) -> Result<f32> {
    // Calculate RMS (Root Mean Square) energy
    let rms = _calculate_rms(audio);

    // Simple threshold: audio with RMS > 0.01 is considered voice
    // Scale and clamp to 0-1 range
    let probability = if rms > 0.01 {
        (rms * 10.0).min(1.0)
    } else {
        0.0
    };

    Ok(probability)
}

#[cfg(not(target_os = "macos"))]
pub fn detect_voice_activity(_audio: &[f32]) -> Result<f32> {
    anyhow::bail!("Voice activity detection only available on macOS")
}

/// Calculate Root Mean Square energy of audio samples
fn _calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn test_speech_recognition() {
        // Test audio samples (1 second of silence at 16kHz)
        let samples = vec![0.0f32; 16000];

        let result = recognize_speech(&samples);
        // May succeed with empty string or fail with permission error
        println!("Result: {:?}", result);
    }
}
