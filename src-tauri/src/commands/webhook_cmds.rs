//! Webhook management commands

use tauri::State;
use tracing::{info, warn};

use crate::config::{NexiBotConfig, WebhookAction, WebhookEndpoint};

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

/// Get the current webhook configuration.
#[tauri::command]
pub async fn get_webhook_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state.config.read().await;
    serde_json::to_value(&config.webhooks).map_err(|e| e.to_string())
}

/// Enable or disable the webhook server.
#[tauri::command]
pub async fn set_webhook_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    let was_webhook_needed = needs_webhook_server(&config);
    config.webhooks.enabled = enabled;
    let is_webhook_needed = needs_webhook_server(&config);
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!(
        "[WEBHOOK] Webhook server {}",
        if enabled { "enabled" } else { "disabled" }
    );

    if !was_webhook_needed && is_webhook_needed {
        // Start the webhook server
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
                warn!("[WEBHOOK] Failed to start webhook server: {}", e);
            }
        });
    } else if was_webhook_needed && !is_webhook_needed {
        // Current server lifecycle is start-only; disabling prevents new starts and blocks logic via config,
        // but an already-running listener may require app restart to fully release the port.
        warn!(
            "[WEBHOOK] Webhook server disable requested; restart app to fully stop active listener"
        );
    }

    Ok(())
}

/// Add a webhook endpoint.
#[tauri::command]
pub async fn add_webhook_endpoint(
    state: State<'_, AppState>,
    name: String,
    action: String,
    target: String,
) -> Result<WebhookEndpoint, String> {
    const MAX_NAME_LEN: usize = 256;
    const MAX_TARGET_LEN: usize = 4096;
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(format!(
            "Endpoint name must be 1–{} characters",
            MAX_NAME_LEN
        ));
    }
    if target.is_empty() || target.len() > MAX_TARGET_LEN {
        return Err(format!(
            "Endpoint target must be 1–{} characters",
            MAX_TARGET_LEN
        ));
    }

    let webhook_action = match action.to_lowercase().as_str() {
        "trigger_task" | "triggertask" => WebhookAction::TriggerTask,
        "send_message" | "sendmessage" => WebhookAction::SendMessage,
        _ => {
            return Err(format!(
                "Unknown action '{}'. Valid: trigger_task, send_message",
                action
            ))
        }
    };

    let endpoint = WebhookEndpoint {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        action: webhook_action,
        target,
    };

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.webhooks.endpoints.push(endpoint.clone());
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());

    info!(
        "[WEBHOOK] Added endpoint: {} ({})",
        endpoint.name, endpoint.id
    );
    Ok(endpoint)
}

/// Remove a webhook endpoint by ID.
#[tauri::command]
pub async fn remove_webhook_endpoint(
    state: State<'_, AppState>,
    endpoint_id: String,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    let before = config.webhooks.endpoints.len();
    config.webhooks.endpoints.retain(|e| e.id != endpoint_id);

    if config.webhooks.endpoints.len() == before {
        return Err(format!("Endpoint not found: {}", endpoint_id));
    }

    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[WEBHOOK] Removed endpoint: {}", endpoint_id);
    Ok(())
}

/// Regenerate the webhook bearer token.
#[tauri::command]
pub async fn regenerate_webhook_token(state: State<'_, AppState>) -> Result<String, String> {
    use rand::Rng;
    let token: String = rand::rngs::OsRng
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.webhooks.auth_token = Some(token.clone());
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());

    info!("[WEBHOOK] Bearer token regenerated");
    Ok(token)
}

/// Enable or disable the Discord bot.
///
/// Saves the config, broadcasts config_changed, and starts the bot immediately
/// when transitioning from disabled → enabled (matching the Telegram pattern).
#[tauri::command]
pub async fn set_discord_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<(), String> {
    let was_enabled = {
        let config = state.config.read().await;
        config.discord.enabled
    };

    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.discord.enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());

    info!(
        "[DISCORD] Discord bot {}",
        if enabled { "enabled" } else { "disabled" }
    );

    // Start the bot immediately when enabling, without waiting for a full config reload.
    if enabled && !was_enabled {
        let app_state = state.inner().clone();
        tokio::spawn(async move {
            if let Err(e) = crate::discord::start_discord_bot(app_state).await {
                warn!("[DISCORD] Failed to start bot after enable: {}", e);
            }
        });
    }

    Ok(())
}

/// Enable or disable the Slack bot.
///
/// Saves the config and broadcasts config_changed. Slack uses the shared webhook
/// server (started by needs_webhook_server), so enable/disable takes effect on
/// the next webhook server cycle or a full save via update_config.
#[tauri::command]
pub async fn set_slack_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.slack.enabled = enabled;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());

    info!(
        "[SLACK] Slack bot {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}
