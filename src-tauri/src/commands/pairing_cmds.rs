//! Tauri commands for DM pairing management.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;
use tracing::info;

use super::AppState;
use crate::pairing::{DmPolicy, PairingRequest};

#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeAllowlist {
    pub telegram: Vec<i64>,
    pub whatsapp: Vec<String>,
    pub channels: HashMap<String, Vec<String>>,
}

/// List all pending pairing requests.
#[tauri::command]
pub async fn list_pairing_requests(
    state: State<'_, AppState>,
) -> Result<Vec<PairingRequest>, String> {
    let mut mgr = state.pairing_manager.write().await;
    Ok(mgr.list_pending())
}

/// Approve a pairing code, adding the sender to the runtime allowlist.
#[tauri::command]
pub async fn approve_pairing_code(
    code: String,
    state: State<'_, AppState>,
) -> Result<PairingRequest, String> {
    let mut mgr = state.pairing_manager.write().await;
    mgr.approve_code(&code).map_err(|e| e.to_string())
}

/// Deny a pairing code, removing the pending request.
#[tauri::command]
pub async fn deny_pairing_code(code: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut mgr = state.pairing_manager.write().await;
    mgr.deny_code(&code).map_err(|e| e.to_string())
}

/// Set the DM policy for Telegram.
#[tauri::command]
pub async fn set_telegram_dm_policy(
    policy: DmPolicy,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.telegram.dm_policy = policy;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[PAIRING] Telegram DM policy set to {:?}", policy);
    Ok(())
}

/// Set the DM policy for WhatsApp.
#[tauri::command]
pub async fn set_whatsapp_dm_policy(
    policy: DmPolicy,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.whatsapp.dm_policy = policy;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[PAIRING] WhatsApp DM policy set to {:?}", policy);
    Ok(())
}

/// Set the DM policy for Discord.
#[tauri::command]
pub async fn set_discord_dm_policy(
    policy: DmPolicy,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.discord.dm_policy = policy;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[PAIRING] Discord DM policy set to {:?}", policy);
    Ok(())
}

/// Set the DM policy for Slack.
#[tauri::command]
pub async fn set_slack_dm_policy(
    policy: DmPolicy,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.slack.dm_policy = policy;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[PAIRING] Slack DM policy set to {:?}", policy);
    Ok(())
}

/// Set the DM policy for Signal.
#[tauri::command]
pub async fn set_signal_dm_policy(
    policy: DmPolicy,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    let previous_config = config.clone();
    config.signal.dm_policy = policy;
    if let Err(e) = config.save() {
        *config = previous_config;
        return Err(e.to_string());
    }
    drop(config);
    let _ = state.config_changed.send(());
    info!("[PAIRING] Signal DM policy set to {:?}", policy);
    Ok(())
}

/// Get the runtime allowlist (approved via pairing).
#[tauri::command]
pub async fn get_runtime_allowlist(state: State<'_, AppState>) -> Result<RuntimeAllowlist, String> {
    let mgr = state.pairing_manager.read().await;
    Ok(RuntimeAllowlist {
        telegram: mgr.get_telegram_allowlist(),
        whatsapp: mgr.get_whatsapp_allowlist(),
        channels: mgr.get_all_channel_allowlists(),
    })
}

/// Remove a sender from the runtime allowlist.
#[tauri::command]
pub async fn remove_from_allowlist(
    channel: String,
    sender_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut mgr = state.pairing_manager.write().await;
    match channel.as_str() {
        "telegram" => {
            let chat_id: i64 = sender_id
                .parse()
                .map_err(|_| "Invalid chat ID".to_string())?;
            mgr.remove_telegram(chat_id).map_err(|e| e.to_string())
        }
        "whatsapp" => mgr.remove_whatsapp(&sender_id).map_err(|e| e.to_string()),
        channel => mgr
            .remove_channel_sender(channel, &sender_id)
            .map_err(|e| e.to_string()),
    }
}
