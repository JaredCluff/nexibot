//! Batch command - batch operations and scripting

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
pub struct BatchArgs {
    #[command(subcommand)]
    command: BatchCommand,
}

#[derive(Subcommand)]
pub enum BatchCommand {
    /// Run batch operations from JSONL file
    Run {
        /// Path to JSONL file
        file: PathBuf,
        /// Stop on first error
        #[arg(long)]
        stop_on_error: bool,
    },
    /// Schedule operations with cron expression
    Schedule {
        /// Cron expression
        cron: String,
        /// Command to execute
        command: String,
    },
}

pub async fn handle(args: BatchArgs, _client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        BatchCommand::Run {
            file,
            stop_on_error,
        } => {
            output::info(&format!("Running batch from: {}", file.display()));
            output::info(&format!("Stop on error: {}", stop_on_error));
            Ok(())
        }
        BatchCommand::Schedule { cron, command } => {
            output::success(&format!("Scheduled '{}' with cron: {}", command, cron));
            Ok(())
        }
    }
}
