//! Tauri commands for API key rotation

use crate::key_rotation::{KeyProvider, KeyRotationManager};
use serde_json::json;
use tauri::State;
use tracing::info;

#[tauri::command]
pub async fn add_api_key(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
    key: String,
    label: Option<String>,
) -> Result<String, String> {
    info!("[KEY_ROTATION_CMD] Adding key for provider: {}", provider);

    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation
        .add_key(provider, key, label, None, vec![])
        .await
}

#[tauri::command]
pub async fn get_active_api_key(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
) -> Result<String, String> {
    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation.get_active_key(&provider).await
}

#[tauri::command]
pub async fn activate_api_key(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
    key_id: String,
) -> Result<(), String> {
    info!(
        "[KEY_ROTATION_CMD] Activating key {} for {}",
        key_id, provider
    );

    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation.activate_key(&provider, &key_id).await
}

#[tauri::command]
pub async fn rotate_api_key(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
    new_key: String,
) -> Result<String, String> {
    info!("[KEY_ROTATION_CMD] Rotating key for {}", provider);

    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation.rotate_key(&provider, new_key).await
}

#[tauri::command]
pub async fn disable_api_key(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
    key_id: String,
) -> Result<(), String> {
    info!(
        "[KEY_ROTATION_CMD] Disabling key {} for {}",
        key_id, provider
    );

    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation.disable_key(&provider, &key_id).await
}

#[tauri::command]
pub async fn list_api_keys(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
) -> Result<Vec<serde_json::Value>, String> {
    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    let keys = key_rotation.list_keys(&provider).await?;

    let result = keys
        .iter()
        .map(|(id, label, expires, usage, is_active)| {
            json!({
                "id": id,
                "label": label,
                "expires_at": expires,
                "usage_count": usage,
                "is_active": is_active,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn get_key_rotation_status(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    let statuses = key_rotation.get_rotation_status().await;

    let result = statuses
        .iter()
        .map(|status| {
            json!({
                "provider": status.provider.to_string(),
                "current_key_id": status.current_key_id,
                "active_key_age_days": status.active_key_age_days,
                "next_rotation": status.next_rotation,
                "expiry_warning": status.expiry_warning,
                "fallback_keys_available": status.fallback_keys_available,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn set_rotation_schedule(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
    provider: String,
    rotate_days: u32,
    warn_days: u32,
    auto_rotate: bool,
) -> Result<(), String> {
    info!(
        "[KEY_ROTATION_CMD] Setting rotation schedule for {}: {} days",
        provider, rotate_days
    );

    let provider = match provider.as_str() {
        "claude" => KeyProvider::Claude,
        "openai" => KeyProvider::OpenAI,
        "anthropic" => KeyProvider::Anthropic,
        "deepgram" => KeyProvider::Deepgram,
        "elevenlabs" => KeyProvider::ElevenLabs,
        name => KeyProvider::Custom(name.to_string()),
    };

    key_rotation
        .set_rotation_schedule(provider, rotate_days, warn_days, auto_rotate)
        .await
}

#[tauri::command]
pub async fn get_key_audit_log(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    let log = key_rotation.get_audit_log().await;

    let result = log
        .iter()
        .map(|entry| {
            json!({
                "timestamp": entry.timestamp,
                "provider": entry.provider.to_string(),
                "key_id": entry.key_id,
                "action": entry.action,
                "details": entry.details,
            })
        })
        .collect();

    Ok(result)
}

#[tauri::command]
pub async fn check_key_expiry_warnings(
    key_rotation: State<'_, std::sync::Arc<KeyRotationManager>>,
) -> Result<Vec<serde_json::Value>, String> {
    let warnings = key_rotation.check_expiry_warnings().await;

    let result = warnings
        .iter()
        .map(|(provider, msg)| {
            json!({
                "provider": provider.to_string(),
                "warning": msg,
            })
        })
        .collect();

    Ok(result)
}
