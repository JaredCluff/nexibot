//! Completion command - shell completion generation

use crate::error::CliError;
use clap::{Parser, Subcommand};

#[derive(Parser)]
pub struct CompletionArgs {
    #[command(subcommand)]
    command: CompletionCommand,
}

#[derive(Subcommand)]
pub enum CompletionCommand {
    /// Generate bash completion
    Bash,
    /// Generate zsh completion
    Zsh,
    /// Generate fish completion
    Fish,
}

pub async fn handle(args: CompletionArgs) -> Result<(), CliError> {
    match args.command {
        CompletionCommand::Bash => {
            println!("# Bash completion for nexibot");
            println!("# Add to ~/.bashrc or ~/.bash_profile:");
            println!("# eval \"$(nexibot completion bash)\"");
            Ok(())
        }
        CompletionCommand::Zsh => {
            println!("# Zsh completion for nexibot");
            println!("# Add to ~/.zshrc:");
            println!("# eval \"$(nexibot completion zsh)\"");
            Ok(())
        }
        CompletionCommand::Fish => {
            println!("# Fish completion for nexibot");
            println!("# Add to ~/.config/fish/config.fish:");
            println!("# nexibot completion fish | source");
            Ok(())
        }
    }
}
