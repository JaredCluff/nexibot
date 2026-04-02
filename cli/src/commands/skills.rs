//! Skills command - discover and execute skills

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
pub struct SkillsArgs {
    #[command(subcommand)]
    command: SkillsCommand,
}

#[derive(Subcommand)]
pub enum SkillsCommand {
    /// List all available skills
    List,
    /// Get skill info
    Info { name: String },
    /// Execute a skill
    Exec {
        name: String,
        #[arg(long)]
        args: Vec<String>,
    },
}

pub async fn handle(args: SkillsArgs, client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        SkillsCommand::List => handle_list(client).await,
        SkillsCommand::Info { name } => handle_info(client, &name).await,
        SkillsCommand::Exec { name, args } => handle_exec(client, &name, args).await,
    }
}

async fn handle_list(client: &NexiBotClient) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = client.list_skills().await?;
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_info(client: &NexiBotClient, name: &str) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    let response = json!({"name": name});
    println!("{}", format_output(&response, client.format()));
    Ok(())
}

async fn handle_exec(
    client: &NexiBotClient,
    name: &str,
    _args: Vec<String>,
) -> Result<(), CliError> {
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }
    output::info(&format!("Executing skill: {}", name));
    Ok(())
}
