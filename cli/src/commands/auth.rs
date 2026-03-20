//! Auth command - manage API tokens

use crate::client::NexiBotClient;
use crate::error::CliError;
use crate::output;
use clap::{Parser, Subcommand};

#[derive(Parser)]
pub struct AuthArgs {
    #[command(subcommand)]
    command: AuthCommand,
}

#[derive(Subcommand)]
pub enum AuthCommand {
    /// Login with API token
    Login { token: String },
    /// Logout (clear local token)
    Logout,
    /// Show current token
    Show,
}

pub async fn handle(args: AuthArgs, _client: &NexiBotClient) -> Result<(), CliError> {
    match args.command {
        AuthCommand::Login { token } => {
            // Guard against panics from direct byte-index slicing on short tokens.
            // Tokens shorter than 12 bytes are shown in full; longer tokens are
            // redacted to first-8 + last-4 characters.
            let display = if token.len() >= 12 {
                format!(
                    "{}...{}",
                    token.get(..8).unwrap_or(&token),
                    token.get(token.len() - 4..).unwrap_or(&token),
                )
            } else {
                token.clone()
            };
            output::success(&format!("Logged in with token: {}", display));
            Ok(())
        }
        AuthCommand::Logout => {
            output::success("Logged out");
            Ok(())
        }
        AuthCommand::Show => {
            output::info("Token: (not shown for security)");
            Ok(())
        }
    }
}
