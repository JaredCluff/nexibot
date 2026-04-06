//! Telegram bot management commands

use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::State;
use tracing::info;

use super::AppState;

#[derive(Debug, Serialize)]
pub struct TelegramStatus {
    pub enabled: bool,
    pub has_token: bool,
    pub bot_running: bool,
    pub last_error: Option<String>,
}

/// Get the current Telegram bot status (enabled, token configured, running).
#[tauri::command]
pub async fn get_telegram_status(state: State<'_, AppState>) -> Result<TelegramStatus, String> {
    let config = state.config.read().await;
    let last_error = state.telegram_last_error.lock().await.clone();
    Ok(TelegramStatus {
        enabled: config.telegram.enabled,
        has_token: !config.telegram.bot_token.is_empty(),
        bot_running: state.telegram_running.load(Ordering::Relaxed),
        last_error,
    })
}

/// Get the current Telegram bot configuration.
#[tauri::command]
pub async fn get_telegram_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.read().await;
    serde_json::to_value(&config.telegram).map_err(|e| e.to_string())
}

/// Enable or disable the Telegram bot.
#[tauri::command]
pub async fn set_telegram_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.telegram.enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!(
        "[TELEGRAM] Telegram bot {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Set the Telegram bot token.
#[tauri::command]
pub async fn set_telegram_bot_token(
    state: State<'_, AppState>,
    token: String,
) -> Result<(), String> {
    let token = {
        let config = state.config.read().await;
        if config.key_vault.intercept_config && state.key_interceptor.is_enabled() {
            state.key_interceptor.intercept_config_string(&token)
        } else {
            token
        }
    };
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.telegram.bot_token = token;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    config.resolve_key_vault_proxies();
    drop(config);
    let _ = state.config_changed.send(());
    info!("[TELEGRAM] Bot token updated");
    Ok(())
}

/// Set allowed Telegram chat IDs.
#[tauri::command]
pub async fn set_telegram_allowed_chat_ids(
    state: State<'_, AppState>,
    chat_ids: Vec<i64>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.telegram.allowed_chat_ids = chat_ids.clone();
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[TELEGRAM] Allowed chat IDs updated: {:?}", chat_ids);
    Ok(())
}

/// Send a test message to the first allowed Telegram chat ID.
/// Uses the Telegram Bot API directly via reqwest.
#[tauri::command]
pub async fn send_telegram_test_message(state: State<'_, AppState>) -> Result<String, String> {
    let (token, chat_ids) = {
        let config = state.config.read().await;
        (
            state
                .key_interceptor
                .restore_config_string(&config.telegram.bot_token),
            config.telegram.allowed_chat_ids.clone(),
        )
    };

    if token.is_empty() {
        return Err("Bot token is not configured".to_string());
    }
    if chat_ids.is_empty() {
        return Err("No allowed chat IDs configured. Add your chat ID first (get it from @userinfobot on Telegram).".to_string());
    }

    let chat_id = chat_ids[0];
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);

    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": "Hello from NexiBot! \u{1f916}\n\nYour Telegram integration is working. You can reply to this message to start chatting.",
        "parse_mode": "Markdown"
    });

    let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(10)).timeout(std::time::Duration::from_secs(30)).build().unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to send message: {}", e))?;

    if resp.status().is_success() {
        info!("[TELEGRAM] Test message sent to chat_id {}", chat_id);
        Ok(format!("Test message sent to chat ID {}", chat_id))
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(format!("Telegram API error ({}): {}", status, text))
    }
}

/// Enable or disable Telegram voice message handling.
#[tauri::command]
pub async fn set_telegram_voice_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.telegram.voice_enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!(
        "[TELEGRAM] Voice messages {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}
