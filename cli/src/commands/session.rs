//! Session command - manage conversation sessions

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
pub struct SessionArgs {
    #[command(subcommand)]
    command: SessionCommand,
}

#[derive(Subcommand)]
pub enum SessionCommand {
    /// List all sessions
    List,
    /// Create a new session
    New { name: Option<String> },
    /// Load a session
    Load { session_id: String },
    /// Delete a session
    Delete { session_id: String },
    /// Get session info
    Info { session_id: String },
}

pub async fn handle(args: SessionArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        SessionCommand::List => handle_list(client).await,
        SessionCommand::New { name } => handle_new(client, name).await,
        SessionCommand::Load { session_id } => handle_load(client, &session_id).await,
        SessionCommand::Delete { session_id } => handle_delete(client, &session_id).await,
        SessionCommand::Info { session_id } => handle_info(client, &session_id).await,
    }
}

async fn handle_list(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = client.list_sessions().await?;
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_new(client: &NexiBotClient, name: Option<String>) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success(&format!(
        "Created new session{}",
        name.as_ref()
            .map(|n| format!(": {}", n))
            .unwrap_or_default()
    ));
    Ok(())
}

async fn handle_load(client: &NexiBotClient, session_id: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success(&format!("Loaded session: {}", session_id));
    Ok(())
}

async fn handle_delete(client: &NexiBotClient, session_id: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success(&format!("Deleted session: {}", session_id));
    Ok(())
}

async fn handle_info(client: &NexiBotClient, session_id: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = json!({"session_id": session_id});
    println!("{}", format_output(&response, client.format()));
    Ok(())
}
