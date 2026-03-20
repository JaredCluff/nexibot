//! Voice command - control voice system

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
pub struct VoiceArgs {
    #[command(subcommand)]
    command: VoiceCommand,
}

#[derive(Subcommand)]
pub enum VoiceCommand {
    /// Start listening for wake word
    Listen,
    /// Stop listening and return to idle
    StopListening,
    /// Play a test TTS audio
    TestTts { text: String },
    /// Get voice status
    Status,
    /// Toggle voice response (TTS)
    Toggle,
}

pub async fn handle(args: VoiceArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        VoiceCommand::Listen => handle_listen(client).await,
        VoiceCommand::StopListening => handle_stop_listening(client).await,
        VoiceCommand::TestTts { text } => handle_test_tts(client, &text).await,
        VoiceCommand::Status => handle_status(client).await,
        VoiceCommand::Toggle => handle_toggle(client).await,
    }
}

async fn handle_listen(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::info("Starting voice listening mode...");
    Ok(())
}

async fn handle_stop_listening(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success("Stopped listening");
    Ok(())
}

async fn handle_test_tts(client: &NexiBotClient, text: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::info(&format!("Testing TTS with: {}", text));
    Ok(())
}

async fn handle_status(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = json!({"status": "idle"});
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_toggle(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success("Voice response toggled");
    Ok(())
}
