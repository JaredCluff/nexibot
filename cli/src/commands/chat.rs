//! Chat command - send messages to Claude

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::Parser;

#[derive(Parser)]
pub struct ChatArgs {
    /// Message to send to Claude
    message: String,

    /// Wait for response (streaming)
    #[arg(short, long)]
    stream: bool,

    /// Include memory context
    #[arg(short, long)]
    with_memory: bool,

    /// Include available skills
    #[arg(short, long)]
    with_skills: bool,

    /// Set thinking budget (for models that support extended thinking)
    #[arg(long)]
    thinking_budget: Option<u32>,
}

pub async fn handle(args: ChatArgs, client: &NexiBotClient) -> Result<(), CliError> {
    // Check server health
    if !client.health_check().await? {
        return Err(CliError::ServerUnreachable);
    }

    // Show loading indicator
    if !matches!(client.format(), "json") {
        print!("Sending message... ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }

    // Send message
    let response = client.send_message(&args.message).await?;

    // Format and display response
    let output = format_output(&response, client.format());
    println!("{}", output);

    if matches!(client.format(), "table") || matches!(client.format(), "plain") {
        if let Some(text) = response.get("response").and_then(|v| v.as_str()) {
            output::success(&format!("Response received ({} chars)", text.len()));
        }
    }

    Ok(())
}
