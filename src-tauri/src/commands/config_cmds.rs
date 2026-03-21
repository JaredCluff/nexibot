//! Configuration management commands

use tauri::State;
use tracing::{error, info, warn};

use crate::config::NexiBotConfig;
use crate::oauth::AuthProfileManager;

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

/// Get current configuration.
///
/// Returns a redacted copy: secret fields (API keys, bot tokens, passwords)
/// are masked so the frontend never sees real credentials. The Settings UI
/// shows masked values; when the user saves, `update_config` receives the
/// masked or new value and the key-vault interceptor handles storage.
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<NexiBotConfig, String> {
    let config = state.config.read().await;
    let mut redacted = config.clone();
    redact_secrets(&mut redacted);
    Ok(redacted)
}

/// Mask a secret string: show first 4 and last 4 chars with `***` in the
/// middle, or just `***` if it's too short. Proxy keys are shown as-is
/// since they are not secrets.
fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    // Proxy keys are safe to expose — they are opaque handles, not real secrets
    if crate::security::key_vault::KeyVault::is_proxy_key(s) {
        return s.to_string();
    }
    // Use char-based indexing to avoid panics on multi-byte UTF-8 boundaries
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        "***".to_string()
    } else {
        let prefix: String = chars[..4].iter().collect();
        let suffix: String = chars[chars.len() - 4..].iter().collect();
        format!("{}***{}", prefix, suffix)
    }
}

fn mask_option(opt: &mut Option<String>) {
    if let Some(ref val) = opt {
        *opt = Some(mask_secret(val));
    }
}

fn mask_string(s: &mut String) {
    if !s.is_empty() {
        *s = mask_secret(s);
    }
}

/// Replace all secret fields in a config with masked versions.
fn redact_secrets(config: &mut NexiBotConfig) {
    mask_option(&mut config.claude.api_key);
    mask_option(&mut config.openai.api_key);
    mask_option(&mut config.cerebras.api_key);
    if let Some(ref mut google) = config.google {
        mask_option(&mut google.api_key);
    }
    if let Some(ref mut deepseek) = config.deepseek {
        mask_option(&mut deepseek.api_key);
    }
    if let Some(ref mut copilot) = config.github_copilot {
        mask_option(&mut copilot.token);
    }
    if let Some(ref mut minimax) = config.minimax {
        mask_option(&mut minimax.api_key);
    }
    if let Some(ref mut qwen) = config.qwen {
        mask_option(&mut qwen.api_key);
    }
    mask_option(&mut config.k2k.private_key_pem);
    mask_option(&mut config.search.brave_api_key);
    mask_option(&mut config.search.tavily_api_key);
    mask_option(&mut config.stt.deepgram_api_key);
    mask_option(&mut config.stt.openai_api_key);
    mask_option(&mut config.tts.elevenlabs_api_key);
    mask_option(&mut config.tts.cartesia_api_key);
    mask_option(&mut config.webhooks.auth_token);
    mask_option(&mut config.webchat.api_key);
    mask_string(&mut config.telegram.bot_token);
    mask_string(&mut config.discord.bot_token);
    mask_string(&mut config.whatsapp.access_token);
    mask_string(&mut config.whatsapp.verify_token);
    mask_string(&mut config.whatsapp.app_secret);
    mask_string(&mut config.slack.bot_token);
    mask_string(&mut config.slack.app_token);
    mask_string(&mut config.slack.signing_secret);
    mask_string(&mut config.teams.app_password);
    mask_string(&mut config.matrix.access_token);
    mask_string(&mut config.email.imap_password);
    mask_string(&mut config.email.smtp_password);
    mask_string(&mut config.gmail.client_secret);
    mask_string(&mut config.gmail.refresh_token);
    mask_string(&mut config.bluebubbles.password);
    mask_string(&mut config.mattermost.bot_token);
    mask_string(&mut config.google_chat.verification_token);
    mask_string(&mut config.google_chat.incoming_webhook_url);
    mask_string(&mut config.messenger.page_access_token);
    mask_string(&mut config.messenger.verify_token);
    mask_string(&mut config.messenger.app_secret);
    mask_string(&mut config.instagram.access_token);
    mask_string(&mut config.instagram.verify_token);
    mask_string(&mut config.instagram.app_secret);
    mask_string(&mut config.line.channel_access_token);
    mask_string(&mut config.line.channel_secret);
    mask_string(&mut config.twilio.auth_token);
    mask_string(&mut config.mastodon.access_token);
    mask_string(&mut config.rocketchat.password);
}

/// Update configuration
#[tauri::command]
pub async fn update_config(
    mut new_config: NexiBotConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    info!("Updating configuration");
    let previous_config = state.config.read().await.clone();
    let was_webhook_needed = needs_webhook_server(&previous_config);
    let is_webhook_needed = needs_webhook_server(&new_config);
    let enable_discord = !previous_config.discord.enabled && new_config.discord.enabled;
    let disable_discord = previous_config.discord.enabled && !new_config.discord.enabled;
    let enable_signal = !previous_config.signal.enabled && new_config.signal.enabled;
    let enable_matrix = !previous_config.matrix.enabled && new_config.matrix.enabled;
    let enable_bluebubbles = !previous_config.bluebubbles.enabled && new_config.bluebubbles.enabled;
    let enable_mattermost = !previous_config.mattermost.enabled && new_config.mattermost.enabled;
    let enable_mastodon = !previous_config.mastodon.enabled && new_config.mastodon.enabled;
    let enable_rocketchat = !previous_config.rocketchat.enabled && new_config.rocketchat.enabled;
    let enable_webchat = !previous_config.webchat.enabled && new_config.webchat.enabled;
    let disable_webchat = previous_config.webchat.enabled && !new_config.webchat.enabled;

    // Restore masked fields: if a secret field still contains the masked
    // value (i.e. the user didn't change it), copy the real value from the
    // current runtime config so we don't overwrite real keys with "sk-a***xyz".
    {
        let current = state.config.read().await;
        restore_if_masked(&current.claude.api_key, &mut new_config.claude.api_key);
        restore_if_masked(&current.openai.api_key, &mut new_config.openai.api_key);
        restore_if_masked(&current.cerebras.api_key, &mut new_config.cerebras.api_key);
        if let (Some(cur_g), Some(new_g)) = (current.google.as_ref(), new_config.google.as_mut()) {
            restore_if_masked(&cur_g.api_key, &mut new_g.api_key);
        }
        if let (Some(cur_d), Some(new_d)) = (current.deepseek.as_ref(), new_config.deepseek.as_mut()) {
            restore_if_masked(&cur_d.api_key, &mut new_d.api_key);
        }
        if let (Some(cur_c), Some(new_c)) = (current.github_copilot.as_ref(), new_config.github_copilot.as_mut()) {
            restore_if_masked(&cur_c.token, &mut new_c.token);
        }
        if let (Some(cur_m), Some(new_m)) = (current.minimax.as_ref(), new_config.minimax.as_mut()) {
            restore_if_masked(&cur_m.api_key, &mut new_m.api_key);
        }
        if let (Some(cur_q), Some(new_q)) = (current.qwen.as_ref(), new_config.qwen.as_mut()) {
            restore_if_masked(&cur_q.api_key, &mut new_q.api_key);
        }
        restore_if_masked(&current.k2k.private_key_pem, &mut new_config.k2k.private_key_pem);
        restore_if_masked(&current.search.brave_api_key, &mut new_config.search.brave_api_key);
        restore_if_masked(&current.search.tavily_api_key, &mut new_config.search.tavily_api_key);
        restore_if_masked(&current.stt.deepgram_api_key, &mut new_config.stt.deepgram_api_key);
        restore_if_masked(&current.stt.openai_api_key, &mut new_config.stt.openai_api_key);
        restore_if_masked(&current.tts.elevenlabs_api_key, &mut new_config.tts.elevenlabs_api_key);
        restore_if_masked(&current.tts.cartesia_api_key, &mut new_config.tts.cartesia_api_key);
        restore_if_masked(&current.webhooks.auth_token, &mut new_config.webhooks.auth_token);
        restore_if_masked(&current.webchat.api_key, &mut new_config.webchat.api_key);
        restore_str_if_masked(&current.telegram.bot_token, &mut new_config.telegram.bot_token);
        restore_str_if_masked(&current.discord.bot_token, &mut new_config.discord.bot_token);
        restore_str_if_masked(&current.whatsapp.access_token, &mut new_config.whatsapp.access_token);
        restore_str_if_masked(&current.whatsapp.verify_token, &mut new_config.whatsapp.verify_token);
        restore_str_if_masked(&current.whatsapp.app_secret, &mut new_config.whatsapp.app_secret);
        restore_str_if_masked(&current.slack.bot_token, &mut new_config.slack.bot_token);
        restore_str_if_masked(&current.slack.app_token, &mut new_config.slack.app_token);
        restore_str_if_masked(&current.slack.signing_secret, &mut new_config.slack.signing_secret);
        restore_str_if_masked(&current.teams.app_password, &mut new_config.teams.app_password);
        restore_str_if_masked(&current.matrix.access_token, &mut new_config.matrix.access_token);
        restore_str_if_masked(&current.email.imap_password, &mut new_config.email.imap_password);
        restore_str_if_masked(&current.email.smtp_password, &mut new_config.email.smtp_password);
        restore_str_if_masked(&current.gmail.client_secret, &mut new_config.gmail.client_secret);
        restore_str_if_masked(&current.gmail.refresh_token, &mut new_config.gmail.refresh_token);
        restore_str_if_masked(&current.bluebubbles.password, &mut new_config.bluebubbles.password);
        restore_str_if_masked(&current.mattermost.bot_token, &mut new_config.mattermost.bot_token);
        restore_str_if_masked(&current.google_chat.verification_token, &mut new_config.google_chat.verification_token);
        restore_str_if_masked(&current.google_chat.incoming_webhook_url, &mut new_config.google_chat.incoming_webhook_url);
        restore_str_if_masked(&current.messenger.page_access_token, &mut new_config.messenger.page_access_token);
        restore_str_if_masked(&current.messenger.verify_token, &mut new_config.messenger.verify_token);
        restore_str_if_masked(&current.messenger.app_secret, &mut new_config.messenger.app_secret);
        restore_str_if_masked(&current.instagram.access_token, &mut new_config.instagram.access_token);
        restore_str_if_masked(&current.instagram.verify_token, &mut new_config.instagram.verify_token);
        restore_str_if_masked(&current.instagram.app_secret, &mut new_config.instagram.app_secret);
        restore_str_if_masked(&current.line.channel_access_token, &mut new_config.line.channel_access_token);
        restore_str_if_masked(&current.line.channel_secret, &mut new_config.line.channel_secret);
        restore_str_if_masked(&current.twilio.auth_token, &mut new_config.twilio.auth_token);
        restore_str_if_masked(&current.mastodon.access_token, &mut new_config.mastodon.access_token);
        restore_str_if_masked(&current.rocketchat.password, &mut new_config.rocketchat.password);
    }

    // Intercept real API keys in config values before persisting.
    // When the vault is enabled, any real key typed into the Settings UI is
    // stored encrypted in the vault and the proxy key is written to config.yaml.
    if new_config.key_vault.intercept_config {
        let interceptor = &state.key_interceptor;
        if interceptor.is_enabled() {
            if let Some(ref key) = new_config.claude.api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.claude.api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.openai.api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.openai.api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.cerebras.api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.cerebras.api_key = Some(sanitized);
            }
            if let Some(google) = new_config.google.as_mut() {
                if let Some(ref key) = google.api_key {
                    let sanitized = interceptor.intercept_config_string(key);
                    google.api_key = Some(sanitized);
                }
            }
            if let Some(deepseek) = new_config.deepseek.as_mut() {
                if let Some(ref key) = deepseek.api_key {
                    let sanitized = interceptor.intercept_config_string(key);
                    deepseek.api_key = Some(sanitized);
                }
            }
            if let Some(github_copilot) = new_config.github_copilot.as_mut() {
                if let Some(ref token) = github_copilot.token {
                    let sanitized = interceptor.intercept_config_string(token);
                    github_copilot.token = Some(sanitized);
                }
            }
            if let Some(minimax) = new_config.minimax.as_mut() {
                if let Some(ref key) = minimax.api_key {
                    let sanitized = interceptor.intercept_config_string(key);
                    minimax.api_key = Some(sanitized);
                }
            }
            if let Some(qwen) = new_config.qwen.as_mut() {
                if let Some(ref key) = qwen.api_key {
                    let sanitized = interceptor.intercept_config_string(key);
                    qwen.api_key = Some(sanitized);
                }
            }
            if let Some(ref key) = new_config.k2k.private_key_pem {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.k2k.private_key_pem = Some(sanitized);
            }
            if let Some(ref key) = new_config.search.brave_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.search.brave_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.search.tavily_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.search.tavily_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.stt.deepgram_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.stt.deepgram_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.stt.openai_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.stt.openai_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.tts.elevenlabs_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.tts.elevenlabs_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.tts.cartesia_api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.tts.cartesia_api_key = Some(sanitized);
            }
            if let Some(ref key) = new_config.webhooks.auth_token {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.webhooks.auth_token = Some(sanitized);
            }
            if !new_config.telegram.bot_token.is_empty() {
                new_config.telegram.bot_token =
                    interceptor.intercept_config_string(&new_config.telegram.bot_token);
            }
            if !new_config.discord.bot_token.is_empty() {
                new_config.discord.bot_token =
                    interceptor.intercept_config_string(&new_config.discord.bot_token);
            }
            if !new_config.whatsapp.access_token.is_empty() {
                new_config.whatsapp.access_token =
                    interceptor.intercept_config_string(&new_config.whatsapp.access_token);
            }
            if !new_config.whatsapp.verify_token.is_empty() {
                new_config.whatsapp.verify_token =
                    interceptor.intercept_config_string(&new_config.whatsapp.verify_token);
            }
            if !new_config.whatsapp.app_secret.is_empty() {
                new_config.whatsapp.app_secret =
                    interceptor.intercept_config_string(&new_config.whatsapp.app_secret);
            }
            if !new_config.slack.bot_token.is_empty() {
                new_config.slack.bot_token =
                    interceptor.intercept_config_string(&new_config.slack.bot_token);
            }
            if !new_config.slack.app_token.is_empty() {
                new_config.slack.app_token =
                    interceptor.intercept_config_string(&new_config.slack.app_token);
            }
            if !new_config.slack.signing_secret.is_empty() {
                new_config.slack.signing_secret =
                    interceptor.intercept_config_string(&new_config.slack.signing_secret);
            }
            if !new_config.teams.app_password.is_empty() {
                new_config.teams.app_password =
                    interceptor.intercept_config_string(&new_config.teams.app_password);
            }
            if !new_config.matrix.access_token.is_empty() {
                new_config.matrix.access_token =
                    interceptor.intercept_config_string(&new_config.matrix.access_token);
            }
            if !new_config.email.imap_password.is_empty() {
                new_config.email.imap_password =
                    interceptor.intercept_config_string(&new_config.email.imap_password);
            }
            if !new_config.email.smtp_password.is_empty() {
                new_config.email.smtp_password =
                    interceptor.intercept_config_string(&new_config.email.smtp_password);
            }
            if !new_config.gmail.client_secret.is_empty() {
                new_config.gmail.client_secret =
                    interceptor.intercept_config_string(&new_config.gmail.client_secret);
            }
            if !new_config.gmail.refresh_token.is_empty() {
                new_config.gmail.refresh_token =
                    interceptor.intercept_config_string(&new_config.gmail.refresh_token);
            }
            if !new_config.bluebubbles.password.is_empty() {
                new_config.bluebubbles.password =
                    interceptor.intercept_config_string(&new_config.bluebubbles.password);
            }
            if !new_config.mattermost.bot_token.is_empty() {
                new_config.mattermost.bot_token =
                    interceptor.intercept_config_string(&new_config.mattermost.bot_token);
            }
            if !new_config.google_chat.verification_token.is_empty() {
                new_config.google_chat.verification_token =
                    interceptor.intercept_config_string(&new_config.google_chat.verification_token);
            }
            if !new_config.google_chat.incoming_webhook_url.is_empty() {
                new_config.google_chat.incoming_webhook_url = interceptor
                    .intercept_config_string(&new_config.google_chat.incoming_webhook_url);
            }
            if !new_config.messenger.page_access_token.is_empty() {
                new_config.messenger.page_access_token =
                    interceptor.intercept_config_string(&new_config.messenger.page_access_token);
            }
            if !new_config.messenger.verify_token.is_empty() {
                new_config.messenger.verify_token =
                    interceptor.intercept_config_string(&new_config.messenger.verify_token);
            }
            if !new_config.messenger.app_secret.is_empty() {
                new_config.messenger.app_secret =
                    interceptor.intercept_config_string(&new_config.messenger.app_secret);
            }
            if !new_config.instagram.access_token.is_empty() {
                new_config.instagram.access_token =
                    interceptor.intercept_config_string(&new_config.instagram.access_token);
            }
            if !new_config.instagram.verify_token.is_empty() {
                new_config.instagram.verify_token =
                    interceptor.intercept_config_string(&new_config.instagram.verify_token);
            }
            if !new_config.instagram.app_secret.is_empty() {
                new_config.instagram.app_secret =
                    interceptor.intercept_config_string(&new_config.instagram.app_secret);
            }
            if !new_config.line.channel_access_token.is_empty() {
                new_config.line.channel_access_token =
                    interceptor.intercept_config_string(&new_config.line.channel_access_token);
            }
            if !new_config.line.channel_secret.is_empty() {
                new_config.line.channel_secret =
                    interceptor.intercept_config_string(&new_config.line.channel_secret);
            }
            if !new_config.twilio.auth_token.is_empty() {
                new_config.twilio.auth_token =
                    interceptor.intercept_config_string(&new_config.twilio.auth_token);
            }
            if !new_config.mastodon.access_token.is_empty() {
                new_config.mastodon.access_token =
                    interceptor.intercept_config_string(&new_config.mastodon.access_token);
            }
            if !new_config.rocketchat.password.is_empty() {
                new_config.rocketchat.password =
                    interceptor.intercept_config_string(&new_config.rocketchat.password);
            }
            if let Some(ref key) = new_config.webchat.api_key {
                let sanitized = interceptor.intercept_config_string(key);
                new_config.webchat.api_key = Some(sanitized);
            }
        }
    }

    // Runtime should keep real secret values even when persisted config stores
    // key-vault proxy tokens.
    let mut runtime_config = new_config.clone();
    runtime_config.resolve_key_vault_proxies();

    // Keep runtime guardrails in sync with persisted config.
    {
        let mut guardrails = state.guardrails.write().await;
        guardrails
            .update_config(runtime_config.guardrails.clone())
            .map_err(|e| e.to_string())?;
    }

    // Sync BrowserManager with new browser config
    {
        let mut browser = state.browser.write().await;
        browser.update_config(runtime_config.browser.clone());
    }

    // Keep defense pipeline in sync with persisted config.
    {
        let mut defense = state.defense_pipeline.write().await;
        defense.update_config(runtime_config.defense.clone());
        defense.update_security_level(runtime_config.guardrails.security_level);
    }

    // Keep GatedShell in sync with persisted config.
    if let Some(ref gs) = state.gated_shell {
        gs.update_config(runtime_config.gated_shell.clone()).await;
    }

    // Keep network policy engine in sync with persisted config.
    state
        .network_policy
        .reload(runtime_config.network_policy.clone())
        .await;

    // Update the shared config (all services read from this via state.config.read().await)
    let mut config = state.config.write().await;
    *config = runtime_config;
    drop(config);

    match new_config.save() {
        Ok(_) => {
            info!("[CONFIG] Configuration updated and saved successfully");
            // Broadcast after a successful save so subscribers only react to durable changes.
            let _ = state.config_changed.send(());

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
                            "[WEBHOOK] Failed to start webhook server after config update: {}",
                            e
                        );
                    }
                });
                info!("[WEBHOOK] Started webhook server after config update");
            } else if was_webhook_needed && !is_webhook_needed {
                warn!(
                    "[WEBHOOK] Webhook-backed integrations disabled via config update; restart app to fully stop active listener"
                );
            }

            if enable_discord {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::discord::start_discord_bot(app_state).await {
                        warn!(
                            "[DISCORD] Failed to start Discord bot after config update: {}",
                            e
                        );
                    }
                });
                info!("[DISCORD] Discord enabled via config update — starting bot");
            } else if disable_discord {
                warn!(
                    "[DISCORD] Discord disabled via config update; restart app to fully stop an active gateway session"
                );
            }

            if enable_signal {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::signal::start_signal_listener(app_state).await {
                        warn!(
                            "[SIGNAL] Failed to start Signal listener after config update: {}",
                            e
                        );
                    }
                });
                info!("[SIGNAL] Signal enabled via config update — starting listener");
            }

            if enable_matrix {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::matrix::start_matrix_sync(app_state).await {
                        warn!(
                            "[MATRIX] Failed to start Matrix sync after config update: {}",
                            e
                        );
                    }
                });
                info!("[MATRIX] Matrix enabled via config update — starting sync loop");
            }

            if enable_bluebubbles {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::bluebubbles::start_bluebubbles_listener(app_state).await
                    {
                        warn!(
                            "[BLUEBUBBLES] Failed to start listener after config update: {}",
                            e
                        );
                    }
                });
                info!("[BLUEBUBBLES] Enabled via config update — starting listener");
            }

            if enable_mattermost {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::mattermost::start_mattermost_bot(app_state).await {
                        warn!(
                            "[MATTERMOST] Failed to start bot after config update: {}",
                            e
                        );
                    }
                });
                info!("[MATTERMOST] Enabled via config update — starting bot");
            }

            if enable_mastodon {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::mastodon::start_mastodon_bot(app_state).await {
                        warn!("[MASTODON] Failed to start bot after config update: {}", e);
                    }
                });
                info!("[MASTODON] Enabled via config update — starting bot");
            }

            if enable_rocketchat {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::rocketchat::start_rocketchat_bot(app_state).await {
                        warn!(
                            "[ROCKETCHAT] Failed to start bot after config update: {}",
                            e
                        );
                    }
                });
                info!("[ROCKETCHAT] Enabled via config update — starting bot");
            }

            if enable_webchat {
                let app_state = state.inner().clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::webchat::start_webchat_server(app_state).await {
                        warn!(
                            "[WEBCHAT] Failed to start server after config update: {}",
                            e
                        );
                    }
                });
                info!("[WEBCHAT] Enabled via config update — starting server");
            } else if disable_webchat {
                info!(
                    "[WEBCHAT] Disabled via config update — listener will stop and active sessions will close"
                );
            }

            Ok(())
        }
        Err(e) => {
            error!("Failed to save configuration: {}", e);
            // Best-effort rollback so runtime behavior matches persisted config
            // when save fails (for example ENOSPC).
            let mut rollback_config = previous_config.clone();
            rollback_config.resolve_key_vault_proxies();

            {
                let mut guardrails = state.guardrails.write().await;
                if let Err(rollback_err) =
                    guardrails.update_config(rollback_config.guardrails.clone())
                {
                    warn!(
                        "[CONFIG] Failed to rollback guardrails after save failure: {}",
                        rollback_err
                    );
                }
            }

            {
                let mut browser = state.browser.write().await;
                browser.update_config(rollback_config.browser.clone());
            }

            {
                let mut defense = state.defense_pipeline.write().await;
                defense.update_config(rollback_config.defense.clone());
                defense.update_security_level(rollback_config.guardrails.security_level);
            }

            if let Some(ref gs) = state.gated_shell {
                gs.update_config(rollback_config.gated_shell.clone()).await;
            }

            let mut config = state.config.write().await;
            *config = rollback_config;
            Err(e.to_string())
        }
    }
}

/// Check if this is the first run (no API key and no OAuth profiles)
#[tauri::command]
pub async fn is_first_run(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config.read().await;

    // Check if Claude API key is actually set (not None and not empty)
    let has_api_key = config
        .claude
        .api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    // Check if OpenAI API key is set
    let has_openai_key = config
        .openai
        .api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    // Check if there are any OAuth profiles configured
    let has_oauth = match AuthProfileManager::load() {
        Ok(manager) => {
            !manager.list_profiles("anthropic").is_empty()
                || !manager.list_profiles("openai").is_empty()
        }
        Err(_) => false,
    };

    // First run if no API keys AND no OAuth profiles
    let is_first = !has_api_key && !has_openai_key && !has_oauth;

    Ok(is_first)
}

/// Returns true if a value looks like a masked secret (contains `***`).
fn is_masked(s: &str) -> bool {
    s.contains("***")
}

/// If the incoming Option<String> contains a masked value, restore the real
/// value from the current runtime config.
fn restore_if_masked(current: &Option<String>, incoming: &mut Option<String>) {
    if let Some(ref val) = incoming {
        if is_masked(val) {
            *incoming = current.clone();
        }
    }
}

/// Same as `restore_if_masked` but for plain String fields.
fn restore_str_if_masked(current: &str, incoming: &mut String) {
    if is_masked(incoming) {
        *incoming = current.to_string();
    }
}
