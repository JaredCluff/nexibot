//! WhatsApp management commands

use tauri::State;
use tracing::{info, warn};

use crate::config::NexiBotConfig;

use super::AppState;

fn needs_webhook_server(config: &NexiBotConfig) -> bool {
    config.webhooks.enabled
        || config.whatsapp.enabled
        || config.slack.enabled
        || config.teams.enabled
        || config.google_chat.enabled
        || config.messenger.enabled
        || config.instagram.enabled
        || config.line.enabled
        || config.twilio.enabled
        || config.webchat.enabled
}

/// Get the current WhatsApp configuration.
#[tauri::command]
pub async fn get_whatsapp_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.read().await;
    serde_json::to_value(&config.whatsapp).map_err(|e| e.to_string())
}

/// Enable or disable WhatsApp integration.
#[tauri::command]
pub async fn set_whatsapp_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    let was_webhook_needed = needs_webhook_server(&config);
    config.whatsapp.enabled = enabled;
    let is_webhook_needed = needs_webhook_server(&config);
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!(
        "[WHATSAPP] WhatsApp integration {}",
        if enabled { "enabled" } else { "disabled" }
    );
    if !was_webhook_needed && is_webhook_needed {
        let app_state = state.inner().clone();
        let config_clone = state.config.clone();
        let scheduler_clone = state.scheduler.clone();
        let claude_clone = state.claude_client.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::webhooks::start_webhook_server(
                config_clone,
                scheduler_clone,
                claude_clone,
                Some(app_state),
            )
            .await
            {
                warn!(
                    "[WEBHOOK] Failed to start webhook server after WhatsApp enable: {}",
                    e
                );
            }
        });
    } else if was_webhook_needed && !is_webhook_needed {
        warn!(
            "[WEBHOOK] WhatsApp disabled via command; restart app to fully stop an active webhook listener"
        );
    }
    Ok(())
}

/// Set the WhatsApp phone number ID.
#[tauri::command]
pub async fn set_whatsapp_phone_number_id(
    state: State<'_, AppState>,
    phone_number_id: String,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.phone_number_id = phone_number_id;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WHATSAPP] Phone number ID updated");
    Ok(())
}

/// Set the WhatsApp access token.
#[tauri::command]
pub async fn set_whatsapp_access_token(
    state: State<'_, AppState>,
    access_token: String,
) -> Result<(), String> {
    let access_token = {
        let config = state.config.read().await;
        if config.key_vault.intercept_config && state.key_interceptor.is_enabled() {
            state.key_interceptor.intercept_config_string(&access_token)
        } else {
            access_token
        }
    };
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.access_token = access_token;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    config.resolve_key_vault_proxies();
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WHATSAPP] Access token updated");
    Ok(())
}

/// Set the WhatsApp webhook verify token.
#[tauri::command]
pub async fn set_whatsapp_verify_token(
    state: State<'_, AppState>,
    verify_token: String,
) -> Result<(), String> {
    let verify_token = {
        let config = state.config.read().await;
        if config.key_vault.intercept_config && state.key_interceptor.is_enabled() {
            state.key_interceptor.intercept_config_string(&verify_token)
        } else {
            verify_token
        }
    };
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.verify_token = verify_token;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    config.resolve_key_vault_proxies();
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WHATSAPP] Verify token updated");
    Ok(())
}

/// Set the WhatsApp app secret used for webhook signature verification.
#[tauri::command]
pub async fn set_whatsapp_app_secret(
    state: State<'_, AppState>,
    app_secret: String,
) -> Result<(), String> {
    let app_secret = {
        let config = state.config.read().await;
        if config.key_vault.intercept_config && state.key_interceptor.is_enabled() {
            state.key_interceptor.intercept_config_string(&app_secret)
        } else {
            app_secret
        }
    };
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.app_secret = app_secret;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    config.resolve_key_vault_proxies();
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WHATSAPP] App secret updated");
    Ok(())
}

/// Set the WhatsApp allowed phone numbers.
#[tauri::command]
pub async fn set_whatsapp_allowed_numbers(
    state: State<'_, AppState>,
    numbers: Vec<String>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.allowed_phone_numbers = numbers;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WHATSAPP] Allowed phone numbers updated");
    Ok(())
}
