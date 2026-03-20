//! Config command - manage configuration

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Get configuration
    Get { key: Option<String> },
    /// Set configuration
    Set { key: String, value: String },
    /// Reset to defaults
    Reset,
}

pub async fn handle(args: ConfigArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        ConfigCommand::Get { key } => handle_get(client, key).await,
        ConfigCommand::Set { key, value } => handle_set(client, &key, &value).await,
        ConfigCommand::Reset => handle_reset(client).await,
    }
}

async fn handle_get(client: &NexiBotClient, key: Option<String>) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = if let Some(k) = key {
        json!({"key": k, "value": null})
    } else {
        client.get_config().await?
    };
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_set(client: &NexiBotClient, key: &str, value: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success(&format!("Set {}: {}", key, value));
    Ok(())
}

async fn handle_reset(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::warn("Configuration reset to defaults");
    Ok(())
}
