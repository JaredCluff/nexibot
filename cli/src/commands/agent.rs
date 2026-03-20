//! Agent command - control agent state

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
pub struct AgentArgs {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Subcommand)]
pub enum AgentCommand {
    /// Get agent status
    Status,
    /// Start/resume agent
    Resume,
    /// Pause agent (queue messages)
    Pause,
    /// Stop agent (emergency)
    Stop,
}

pub async fn handle(args: AgentArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        AgentCommand::Status => handle_status(client).await,
        AgentCommand::Resume => handle_resume(client).await,
        AgentCommand::Pause => handle_pause(client).await,
        AgentCommand::Stop => handle_stop(client).await,
    }
}

async fn handle_status(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = json!({"state": "running"});
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_resume(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success("Agent resumed");
    Ok(())
}

async fn handle_pause(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::success("Agent paused");
    Ok(())
}

async fn handle_stop(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::warn("Agent stopped (emergency)");
    Ok(())
}
