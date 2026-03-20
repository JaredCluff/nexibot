//! Subscription management commands

use crate::subscription::{ServiceProvider, Subscription, SubscriptionStatus};
use tauri::State;

use super::AppState;

/// Check subscription status for a provider
#[tauri::command]
pub async fn check_subscription(
    provider: String,
    state: State<'_, AppState>,
) -> Result<SubscriptionStatus, String> {
    let provider_enum = match provider.as_str() {
        "anthropic" => ServiceProvider::Anthropic,
        "openai" => ServiceProvider::OpenAI,
        "deepgram" => ServiceProvider::Deepgram,
        "elevenlabs" => ServiceProvider::ElevenLabs,
        "cartesia" => ServiceProvider::Cartesia,
        _ => return Err(format!("Unknown provider: {}", provider)),
    };

    let manager = state.subscription_manager.read().await;
    manager
        .check_subscription(provider_enum)
        .await
        .map_err(|e| e.to_string())
}

/// Get or provision API credentials for a subscribed service
#[tauri::command]
pub async fn get_subscription_credentials(
    provider: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let provider_enum = match provider.as_str() {
        "anthropic" => ServiceProvider::Anthropic,
        "openai" => ServiceProvider::OpenAI,
        "deepgram" => ServiceProvider::Deepgram,
        "elevenlabs" => ServiceProvider::ElevenLabs,
        "cartesia" => ServiceProvider::Cartesia,
        _ => return Err(format!("Unknown provider: {}", provider)),
    };

    let manager = state.subscription_manager.read().await;
    let credentials = manager
        .get_credentials(provider_enum)
        .await
        .map_err(|e| e.to_string())?;

    Ok(credentials.api_key)
}

/// List all active subscriptions
#[tauri::command]
pub async fn list_subscriptions(state: State<'_, AppState>) -> Result<Vec<Subscription>, String> {
    let manager = state.subscription_manager.read().await;
    Ok(manager.list_subscriptions().await)
}

/// Open subscription portal in browser
#[tauri::command]
pub async fn open_subscription_portal(
    provider: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let provider_enum = provider.as_ref().and_then(|p| match p.as_str() {
        "anthropic" => Some(ServiceProvider::Anthropic),
        "openai" => Some(ServiceProvider::OpenAI),
        "deepgram" => Some(ServiceProvider::Deepgram),
        "elevenlabs" => Some(ServiceProvider::ElevenLabs),
        "cartesia" => Some(ServiceProvider::Cartesia),
        _ => None,
    });

    let manager = state.subscription_manager.read().await;
    manager
        .open_subscription_portal(provider_enum)
        .map_err(|e| e.to_string())
}

/// Refresh all subscriptions
#[tauri::command]
pub async fn refresh_subscriptions(state: State<'_, AppState>) -> Result<(), String> {
    let manager = state.subscription_manager.read().await;
    manager
        .refresh_subscriptions()
        .await
        .map_err(|e| e.to_string())
}
