//! Channel adapter trait for abstracting message sources.
//!
//! Enables reuse of the Claude pipeline across GUI, Telegram, Discord, etc.
//! Provides concrete implementations for each channel.

use async_trait::async_trait;
use tauri::Emitter;
use teloxide::requests::Requester;
use tracing::warn;

use crate::router;

/// Identifies the source channel for a message.
#[derive(Debug, Clone)]
pub enum ChannelSource {
    /// GUI / Tauri frontend
    Gui,
    /// Telegram bot
    Telegram { chat_id: i64 },
    /// WhatsApp Cloud API
    WhatsApp { phone_number: String },
    /// Discord bot
    Discord {
        channel_id: u64,
        #[allow(dead_code)]
        guild_id: Option<u64>,
    },
    /// Slack Events API
    Slack { channel_id: String },
    /// Signal messaging via Signal CLI REST API
    Signal { phone_number: String },
    /// Microsoft Teams via Bot Framework
    Teams { conversation_id: String },
    /// Matrix via Client-Server API
    Matrix { room_id: String },
    /// Inter-agent session
    InterAgent { agent_id: String },
    /// iMessage via BlueBubbles server
    BlueBubbles { chat_guid: String },
    /// Google Chat / Google Workspace
    GoogleChat { space_id: String, sender_id: String },
    /// Mattermost bot
    Mattermost { channel_id: String },
    /// Facebook Messenger
    Messenger { sender_id: String },
    /// Instagram Direct Messages
    Instagram { sender_id: String },
    /// LINE Messaging API
    Line {
        user_id: String,
        conversation_id: String,
    },
    /// Twilio SMS/MMS
    Twilio { phone_number: String },
    /// Mastodon federated social
    Mastodon { account_id: String },
    /// Rocket.Chat bot
    RocketChat { room_id: String },
    /// Self-hosted WebChat browser widget
    WebChat { session_id: String },
    /// Email via IMAP/SMTP
    Email { thread_id: String },
    /// Gmail via Google Gmail API
    Gmail { thread_id: String },
    /// Voice interaction (wake word or push-to-talk)
    Voice,
}

/// Trait for delivering responses back to a channel.
/// Each channel adapter implements this to handle response delivery.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Send the final complete response
    async fn send_response(&self, text: &str) -> Result<(), String>;

    /// Send an error message
    async fn send_error(&self, error: &str) -> Result<(), String>;

    /// Get the channel source identifier
    #[allow(dead_code)]
    fn source(&self) -> &ChannelSource;
}

// ---------------------------------------------------------------------------
// Concrete ChannelAdapter implementations
// ---------------------------------------------------------------------------

/// GUI adapter that emits Tauri events to the frontend.
#[allow(dead_code)]
pub struct GuiAdapter {
    source: ChannelSource,
    window: tauri::Window,
}

impl GuiAdapter {
    #[allow(dead_code)]
    pub fn new(window: tauri::Window) -> Self {
        Self {
            source: ChannelSource::Gui,
            window,
        }
    }
}

#[async_trait]
impl ChannelAdapter for GuiAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        self.window
            .emit(
                "chat:complete",
                serde_json::json!({
                    "text": text,
                }),
            )
            .map_err(|e| e.to_string())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        self.window
            .emit(
                "chat:error",
                serde_json::json!({
                    "error": error,
                }),
            )
            .map_err(|e| e.to_string())
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

/// Telegram adapter that sends messages via the Telegram Bot API.
#[allow(dead_code)]
pub struct TelegramAdapter {
    source: ChannelSource,
    bot: teloxide::Bot,
    chat_id: teloxide::types::ChatId,
}

impl TelegramAdapter {
    #[allow(dead_code)]
    pub fn new(bot: teloxide::Bot, chat_id: teloxide::types::ChatId) -> Self {
        let raw_id = chat_id.0;
        Self {
            source: ChannelSource::Telegram { chat_id: raw_id },
            bot,
            chat_id,
        }
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let response = router::extract_text_from_response(text);
        if response.is_empty() {
            self.bot
                .send_message(self.chat_id, "(No response)")
                .await
                .map_err(|e| e.to_string())?;
            return Ok(());
        }

        for chunk in router::split_message(&response, 4096) {
            if let Err(e) = self.bot.send_message(self.chat_id, &chunk).await {
                warn!("[TELEGRAM] Failed to send chunk: {}", e);
            }
        }
        Ok(())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        self.bot
            .send_message(self.chat_id, format!("Error: {}", error))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

/// WhatsApp adapter that sends messages via the WhatsApp Cloud API.
#[allow(dead_code)]
pub struct WhatsAppAdapter {
    source: ChannelSource,
    app_state: crate::commands::AppState,
    phone: String,
}

impl WhatsAppAdapter {
    #[allow(dead_code)]
    pub fn new(app_state: crate::commands::AppState, phone: String) -> Self {
        Self {
            source: ChannelSource::WhatsApp {
                phone_number: phone.clone(),
            },
            app_state,
            phone,
        }
    }
}

#[async_trait]
impl ChannelAdapter for WhatsAppAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        let response = router::extract_text_from_response(text);
        if response.is_empty() {
            send_whatsapp_text(&self.app_state, &self.phone, "(No response)").await;
            return Ok(());
        }

        for chunk in router::split_message(&response, 4096) {
            send_whatsapp_text(&self.app_state, &self.phone, &chunk).await;
        }
        Ok(())
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        send_whatsapp_text(&self.app_state, &self.phone, &format!("Error: {}", error)).await;
        Ok(())
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}

/// Internal helper -- sends a WhatsApp text message via the Cloud API.
#[allow(dead_code)]
async fn send_whatsapp_text(app_state: &crate::commands::AppState, to: &str, text: &str) {
    let (phone_number_id, access_token) = {
        let config = app_state.config.read().await;
        (
            config.whatsapp.phone_number_id.clone(),
            app_state
                .key_interceptor
                .restore_config_string(&config.whatsapp.access_token),
        )
    };

    if phone_number_id.is_empty() || access_token.is_empty() {
        return;
    }
    let phone_id = phone_number_id;
    let token = access_token;

    let client = reqwest::Client::new();
    let url = format!("https://graph.facebook.com/v18.0/{}/messages", phone_id);
    let body = serde_json::json!({
        "messaging_product": "whatsapp",
        "to": to,
        "type": "text",
        "text": { "body": text },
    });

    if let Err(e) = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&body)
        .send()
        .await
    {
        warn!("[WHATSAPP] Failed to send message: {}", e);
    }
}

/// Voice adapter that sends text to the TTS pipeline.
#[allow(dead_code)]
pub struct VoiceAdapter {
    source: ChannelSource,
    text_tx: tokio::sync::mpsc::Sender<String>,
}

impl VoiceAdapter {
    #[allow(dead_code)]
    pub fn new(text_tx: tokio::sync::mpsc::Sender<String>) -> Self {
        Self {
            source: ChannelSource::Voice,
            text_tx,
        }
    }
}

#[async_trait]
impl ChannelAdapter for VoiceAdapter {
    async fn send_response(&self, text: &str) -> Result<(), String> {
        self.text_tx
            .send(text.to_string())
            .await
            .map_err(|e| format!("Failed to send to TTS: {}", e))
    }

    async fn send_error(&self, error: &str) -> Result<(), String> {
        self.text_tx
            .send(format!("Error: {}", error))
            .await
            .map_err(|e| format!("Failed to send error to TTS: {}", e))
    }

    fn source(&self) -> &ChannelSource {
        &self.source
    }
}
