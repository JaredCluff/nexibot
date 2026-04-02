//! NATS messaging bus integration for NexiBot.
//!
//! Connects to a NATS server and subscribes to inbound messages,
//! routing them through the same Claude pipeline as other channels.
//! Enables inter-agent communication with Animus, Claude Code,
//! OpenCode, and other NATS-connected agents.
//!
//! Subject convention: `{target}.in.{from}`
//!   - `nexibot.in.animus` → message from Animus to NexiBot
//!   - `animus.in.nexibot` → message from NexiBot to Animus

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_nats::Client;
use futures_util::StreamExt;
use tracing::{info, warn};

use crate::channel::ChannelSource;
use crate::commands::AppState;
use crate::router::{self, IncomingMessage, RouteOptions};
use crate::session_overrides::SessionOverrides;
use crate::tool_loop::{NoOpObserver, ToolLoopConfig};

const AGENT_ID: &str = "nexibot";

/// Start the NATS listener. Returns `Ok(())` immediately if NATS is disabled.
pub async fn start_nats_listener(app_state: AppState) -> Result<()> {
    let config = app_state.config.read().await;
    if !config.nats.enabled {
        info!("[NATS] NATS integration disabled in config");
        return Ok(());
    }

    let nats_url = resolve_nats_url(&config.nats.url);
    let inbound_subject = config.nats.inbound_subject.clone();
    drop(config);

    info!("[NATS] Connecting to {}", nats_url);
    let client = async_nats::connect(&nats_url).await.map_err(|e| {
        anyhow::anyhow!("NATS connection to {} failed: {}", nats_url, e)
    })?;

    info!(
        "[NATS] Connected, subscribing to {}",
        inbound_subject
    );
    let mut subscriber = client.subscribe(inbound_subject.clone()).await.map_err(|e| {
        anyhow::anyhow!("NATS subscribe to {} failed: {}", inbound_subject, e)
    })?;

    let client = Arc::new(client);

    // Share client with nats_publish tool
    {
        let mut shared = app_state.nats_publish_client.lock().await;
        *shared = Some(client.clone());
    }

    info!(
        "[NATS] Listener started (server: {}, subject: {})",
        nats_url, inbound_subject
    );

    // Main message loop
    while let Some(msg) = subscriber.next().await {
        // Allow runtime disabling without restart.
        {
            let config = app_state.config.read().await;
            if !config.nats.enabled {
                info!("[NATS] Integration disabled at runtime, stopping listener");
                break;
            }
        }

        let payload = String::from_utf8_lossy(&msg.payload).to_string();

        // Extract sender from subject leaf (e.g., nexibot.in.animus → animus)
        let sender = msg
            .subject
            .as_str()
            .rsplit('.')
            .next()
            .unwrap_or("unknown")
            .to_string();

        // Parse optional JSON wrapper with conversation-id
        let (text, _conversation_id) = parse_payload(&payload);

        info!(
            "[NATS] Received from {} on {}: {}",
            sender,
            msg.subject,
            &text[..text.len().min(200)]
        );

        let app_state_clone = app_state.clone();
        let client_clone = client.clone();
        let subject = msg.subject.clone();
        let sender_clone = sender.clone();

        // Process each message in a spawned task to avoid blocking the subscriber
        tokio::spawn(async move {
            handle_nats_message(
                &app_state_clone,
                &client_clone,
                &subject,
                &sender_clone,
                &text,
            )
            .await;
        });
    }

    Ok(())
}

/// Handle a single inbound NATS message by routing it through the LLM pipeline.
async fn handle_nats_message(
    app_state: &AppState,
    client: &Client,
    subject: &async_nats::Subject,
    sender: &str,
    text: &str,
) {
    let message = IncomingMessage {
        text: text.to_string(),
        channel: ChannelSource::Nats {
            sender: sender.to_string(),
        },
        agent_id: None,
        metadata: HashMap::new(),
    };

    let observer = NoOpObserver;
    let loop_config = ToolLoopConfig {
        max_iterations: 15,
        timeout: Some(std::time::Duration::from_secs(300)),
        max_output_bytes: 10 * 1024 * 1024,
        max_tool_result_bytes: Some(8_000),
        force_summary_on_exhaustion: true,
        channel: Some(ChannelSource::Nats {
            sender: sender.to_string(),
        }),
        run_defense_checks: true,
        streaming: false,
        sender_id: Some(sender.to_string()),
        between_tool_delay_ms: 0,
    };

    let result = {
        let client_guard = app_state.claude_client.read().await;
        let options = RouteOptions {
            claude_client: &*client_guard,
            overrides: SessionOverrides::default(),
            loop_config,
            observer: &observer,
            streaming: false,
            window: None,
            on_stream_chunk: None,
            auto_compact: true,
            save_to_memory: true,
            sync_supermemory: false,
            check_sensitive_data: true,
        };
        router::route_message(&message, options, app_state).await
    };

    match result {
        Ok(routed) => {
            let response = router::extract_text_from_response(&routed.text);
            if response.is_empty() {
                return;
            }

            // Reply on the sender's inbound subject: {sender}.in.{agent_id}
            let reply_subject = format!("{}.in.{}", sender, AGENT_ID);
            if let Err(e) = client
                .publish(reply_subject.clone(), response.into())
                .await
            {
                warn!("[NATS] Failed to publish reply to {}: {}", reply_subject, e);
                return;
            }
            if let Err(e) = client.flush().await {
                warn!("[NATS] Flush after reply failed: {}", e);
            }
            info!(
                "[NATS] Replied to {} on {}",
                sender, reply_subject
            );
        }
        Err(e) => {
            warn!("[NATS] Failed to route message from {}: {:?}", sender, e);
            // Attempt to send error back
            let reply_subject = format!("{}.in.{}", sender, AGENT_ID);
            let err_msg = format!("Error processing message: {}", e);
            let _ = client
                .publish(reply_subject, err_msg.into())
                .await;
            let _ = client.flush().await;
        }
    }
}

/// Parse a NATS payload, extracting the text and optional conversation ID
/// from a JSON wrapper.
fn parse_payload(raw: &str) -> (String, Option<String>) {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(payload) = parsed.get("payload").and_then(|v| v.as_str()) {
            let conv_id = parsed
                .get("x-conversation-id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            return (payload.to_string(), conv_id);
        }
    }
    (raw.to_string(), None)
}

/// Resolve the NATS URL, checking env vars as fallbacks.
fn resolve_nats_url(configured: &str) -> String {
    if !configured.is_empty() && configured != "nats://localhost:14222" {
        return configured.to_string();
    }
    if let Ok(url) = std::env::var("NEXIBOT_NATS_URL") {
        if !url.is_empty() {
            return url;
        }
    }
    if let Ok(url) = std::env::var("ANIMUS_NATS_URL") {
        if !url.is_empty() {
            return url;
        }
    }
    configured.to_string()
}
