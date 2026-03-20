//! Voice service and platform info commands

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::{error, info};

use crate::platform;

use super::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub state: String,
    pub stt_backend: String,
    pub tts_backend: String,
    pub is_sleeping: bool,
    pub voice_response_enabled: bool,
    pub wakeword_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlatformInfo {
    pub os: String,
    pub available_stt_backends: Vec<String>,
    pub available_tts_backends: Vec<String>,
}

/// Start voice service
#[tauri::command]
pub async fn start_voice_service(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    info!("Starting voice service");

    let mut voice = state.voice_service.write().await;
    voice.set_app_handle(app);

    match voice.start().await {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("Failed to start voice service: {}", e);
            Err(e.to_string())
        }
    }
}

/// Stop voice service
#[tauri::command]
pub async fn stop_voice_service(state: State<'_, AppState>) -> Result<(), String> {
    info!("Stopping voice service");

    let mut voice = state.voice_service.write().await;

    match voice.stop().await {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("Failed to stop voice service: {}", e);
            Err(e.to_string())
        }
    }
}

/// Get voice service status
#[tauri::command]
pub async fn get_voice_status(state: State<'_, AppState>) -> Result<VoiceStatus, String> {
    let voice = state.voice_service.read().await;
    let wakeword_enabled = state.config.read().await.wakeword.enabled;

    Ok(VoiceStatus {
        state: format!("{:?}", voice.get_state().await),
        stt_backend: voice.get_stt_backend().await,
        tts_backend: voice.get_tts_backend().await,
        is_sleeping: voice.is_sleeping().await,
        voice_response_enabled: voice.get_voice_response_enabled(),
        wakeword_enabled,
    })
}

/// Test STT by recording from microphone and transcribing
#[tauri::command]
pub async fn test_stt(state: State<'_, AppState>) -> Result<String, String> {
    info!("Testing STT");

    let voice = state.voice_service.read().await;

    match voice.test_stt().await {
        Ok(transcript) => Ok(transcript),
        Err(e) => {
            error!("STT test failed: {}", e);
            Err(e.to_string())
        }
    }
}

/// Test TTS by speaking a message
#[tauri::command]
pub async fn test_tts(
    text: String,
    voice: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("Testing TTS: {} (voice: {:?})", text, voice);

    let voice_service = state.voice_service.read().await;

    match voice_service.test_tts(&text, voice).await {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("TTS test failed: {}", e);
            Err(e.to_string())
        }
    }
}

/// Get platform information including available backends
#[tauri::command]
pub async fn get_platform_info() -> Result<PlatformInfo, String> {
    Ok(PlatformInfo {
        os: platform::current_platform().to_string(),
        available_stt_backends: platform::available_stt_backends()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        available_tts_backends: platform::available_tts_backends()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    })
}

/// Push-to-talk: capture audio for the specified duration (seconds), transcribe, send to Claude, return response
#[tauri::command]
pub async fn push_to_talk(
    duration_secs: Option<u32>,
    state: State<'_, AppState>,
) -> Result<PushToTalkResponse, String> {
    let duration = duration_secs.unwrap_or(5);
    info!("Push-to-talk: capturing {}s of audio", duration);

    // Capture audio (blocking)
    let samples = crate::voice::VoiceService::ptt_capture(duration)
        .map_err(|e| format!("Audio capture failed: {}", e))?;

    if samples.is_empty() {
        return Err("No audio captured from microphone".to_string());
    }
    info!("Push-to-talk: captured {} samples", samples.len());

    // Transcribe
    let voice = state.voice_service.read().await;
    let transcript = voice
        .ptt_transcribe(&samples)
        .await
        .map_err(|e| format!("Transcription failed: {}", e))?;

    if transcript.trim().is_empty() {
        return Ok(PushToTalkResponse {
            transcript: String::new(),
            response: String::new(),
            error: Some("No speech detected".to_string()),
        });
    }
    info!("Push-to-talk: transcribed: {}", transcript);

    // Send to Claude (voice-optimized)
    let response = voice
        .ptt_send_to_claude(&transcript)
        .await
        .map_err(|e| format!("Claude request failed: {}", e))?;

    // Strip markdown and speak the response via TTS
    let clean_response = crate::voice::strip_markdown_for_tts(&response);
    if !clean_response.is_empty() {
        if let Err(e) = voice.speak_text(&clean_response).await {
            error!("PTT TTS playback failed: {}", e);
        }
    }

    Ok(PushToTalkResponse {
        transcript,
        response,
        error: None,
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushToTalkResponse {
    pub transcript: String,
    pub response: String,
    pub error: Option<String>,
}

/// PTT start: begin continuous audio capture (press-and-hold)
#[tauri::command]
pub async fn ptt_start(state: State<'_, AppState>) -> Result<(), String> {
    info!("PTT: starting capture");

    // Check if already recording
    {
        let capture = state.ptt_capture.read().await;
        if capture.is_some() {
            return Err("PTT capture already in progress".to_string());
        }
    }

    let handle = crate::voice::VoiceService::ptt_start_capture()
        .map_err(|e| format!("Failed to start PTT capture: {}", e))?;

    let mut capture = state.ptt_capture.write().await;
    *capture = Some(handle);

    Ok(())
}

/// PTT stop: stop capture, transcribe, send to Claude, return response
#[tauri::command]
pub async fn ptt_stop(state: State<'_, AppState>) -> Result<PushToTalkResponse, String> {
    info!("PTT: stopping capture");

    let handle = {
        let mut capture = state.ptt_capture.write().await;
        match capture.take() {
            Some(h) => h,
            None => {
                info!("PTT: no capture in progress, ignoring stop");
                return Ok(PushToTalkResponse {
                    transcript: String::new(),
                    response: String::new(),
                    error: Some("No speech detected".to_string()),
                });
            }
        }
    };

    let samples = handle
        .stop()
        .map_err(|e| format!("Failed to stop PTT capture: {}", e))?;

    if samples.is_empty() {
        return Ok(PushToTalkResponse {
            transcript: String::new(),
            response: String::new(),
            error: Some("No speech detected".to_string()),
        });
    }
    info!("PTT: captured {} samples", samples.len());

    // Transcribe
    let voice = state.voice_service.read().await;
    let transcript = voice
        .ptt_transcribe(&samples)
        .await
        .map_err(|e| format!("Transcription failed: {}", e))?;

    if transcript.trim().is_empty() {
        return Ok(PushToTalkResponse {
            transcript: String::new(),
            response: String::new(),
            error: Some("No speech detected".to_string()),
        });
    }
    info!("PTT: transcribed: {}", transcript);

    // Send to Claude (voice-optimized)
    let response = voice
        .ptt_send_to_claude(&transcript)
        .await
        .map_err(|e| format!("Claude request failed: {}", e))?;

    // Strip markdown and speak the response via TTS
    let clean_response = crate::voice::strip_markdown_for_tts(&response);
    if !clean_response.is_empty() {
        if let Err(e) = voice.speak_text(&clean_response).await {
            error!("PTT TTS playback failed: {}", e);
        }
    }

    Ok(PushToTalkResponse {
        transcript,
        response,
        error: None,
    })
}

/// PTT cancel: stop capture without processing
#[tauri::command]
pub async fn ptt_cancel(state: State<'_, AppState>) -> Result<(), String> {
    info!("PTT: cancelling capture");

    let mut capture = state.ptt_capture.write().await;
    if let Some(handle) = capture.take() {
        let _ = handle.stop(); // Discard samples
    }

    Ok(())
}

// Legacy commands (kept for backwards compatibility)
/// Start audio capture (legacy - use start_voice_service)
#[tauri::command]
pub async fn start_audio_capture(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    start_voice_service(state, app).await
}

/// Stop audio capture (legacy - use stop_voice_service)
#[tauri::command]
pub async fn stop_audio_capture(state: State<'_, AppState>) -> Result<(), String> {
    stop_voice_service(state).await
}

/// Set voice response (TTS) enabled at runtime and persist to config
#[tauri::command]
pub async fn set_voice_response_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let voice = state.voice_service.read().await;
    let previous_enabled = voice.get_voice_response_enabled();
    voice.set_voice_response_enabled(enabled);
    drop(voice);
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.wakeword.voice_response_enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        drop(config);
        let voice = state.voice_service.read().await;
        voice.set_voice_response_enabled(previous_enabled);
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    Ok(())
}

/// Get current voice response (TTS) enabled state
#[tauri::command]
pub async fn get_voice_response_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let voice = state.voice_service.read().await;
    Ok(voice.get_voice_response_enabled())
}

/// Force return to wake-word idle mode; cancels any active pipeline
#[tauri::command]
pub async fn voice_stop_listening(state: State<'_, AppState>) -> Result<(), String> {
    let voice = state.voice_service.read().await;
    voice.force_stop().await.map_err(|e| e.to_string())
}

/// Set wake word (ONNX) detection enabled and persist to config.
/// Does NOT restart the voice service — caller handles restart if needed.
#[tauri::command]
pub async fn voice_set_wakeword_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.wakeword.enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    Ok(())
}
