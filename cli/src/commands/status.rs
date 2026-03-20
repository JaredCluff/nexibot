//! Status command - server health and status

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output::{self, format_output};
use clap::Parser;
use serde_json::json;

#[derive(Parser)]
pub struct StatusArgs {
    /// Show detailed status
    #[arg(short, long)]
    detailed: bool,
}

pub async fn handle(args: StatusArgs, client: &NexiBotClient) -> Result<(), CliError> {
    if client.health_check().await? {
        output::success("NexiBot server is running");

        if args.detailed {
            let response = json!({
                "status": "running",
                "uptime": "calculating...",
                "version": "0.6.0",
                "api_url": client.base_url(),
            });
            println!("{}", format_output(&response, client.format()));
        }
        Ok(())
    } else {
        output::error("NexiBot server is not reachable");
        Err(CliError::ServerUnreachable)
    }
}
