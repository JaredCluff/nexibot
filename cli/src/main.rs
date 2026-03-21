//! NexiBot Comprehensive CLI
//!
//! Control NexiBot from the command line with full support for:
//! - Chat and message handling
//! - Configuration management
//! - Memory search and management
//! - Voice control (wake word, TTS, listening)
//! - Session management
//! - Skills discovery and execution
//! - Agent control and orchestration
//! - Scripting and automation
//! - Batch operations
//! - JSON output for integration
//!
//! # Examples
//!
//! ```bash
//! # Send a message
//! nexibot chat "What is the weather today?"
//!
//! # Search memory
//! nexibot memory search "my preferences"
//!
//! # Control voice
//! nexibot voice stop-listening
//!
//! # Get config
//! nexibot config get --format json
//!
//! # List sessions
//! nexibot session list
//!
//! # Execute a skill
//! nexibot skills exec --name "search_web" --args query="climate change"
//!
//! # Agent control
//! nexibot agent pause
//!
//! # Batch operations
//! nexibot batch run script.jsonl
//! ```

use clap::{Parser, Subcommand};
use colored::Colorize;
use std::path::PathBuf;
use tracing::info;

mod client;
mod commands;
mod config;
mod error;
mod output;
mod utils;

use client::NexiBotClient;
use config::CliConfig;
use error::CliError;

#[derive(Parser)]
#[command(name = "nexibot")]
#[command(version = "0.1.0")]
#[command(about = "NexiBot Comprehensive CLI - Control your AI agent from the command line", long_about = None)]
#[command(author = "Jared Cluff")]
struct Cli {
    /// NexiBot API server URL (default: http://localhost:18791)
    #[arg(long, global = true, default_value = "http://localhost:18791")]
    api_url: String,

    /// API authentication token
    #[arg(long, global = true)]
    token: Option<String>,

    /// Output format (json, yaml, table, plain)
    #[arg(
        long,
        global = true,
        value_parser = ["json", "yaml", "table", "plain"],
        default_value = "table"
    )]
    format: String,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Config file path (default: platform config directory)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Chat with Claude
    Chat(commands::chat::ChatArgs),

    /// Manage memory
    Memory(commands::memory::MemoryArgs),

    /// Control voice system
    Voice(commands::voice::VoiceArgs),

    /// Manage sessions
    Session(commands::session::SessionArgs),

    /// Manage skills
    Skills(commands::skills::SkillsArgs),

    /// Manage agent state
    Agent(commands::agent::AgentArgs),

    /// Manage configuration
    Config(commands::config::ConfigArgs),

    /// List and manage API tokens
    Auth(commands::auth::AuthArgs),

    /// Batch operations and scripting
    Batch(commands::batch::BatchArgs),

    /// Run security audits and checks
    Security(commands::security::SecurityArgs),

    /// Server health and status
    Status(commands::status::StatusArgs),

    /// Shell completion generation
    Completion(commands::completion::CompletionArgs),

    /// List all available commands (for exploration)
    Help2(commands::help::HelpArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging based on verbosity
    let log_level = if cli.verbose {
        "nexibot_cli=debug,info"
    } else {
        "nexibot_cli=warn,error"
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_env_filter(log_level)
        .init();

    // Load config
    let config = CliConfig::load(&cli.config)?;

    // Create client
    let client = NexiBotClient::new(
        cli.api_url.clone(),
        cli.token.clone().or_else(|| config.token.clone()),
        cli.format.clone(),
    );

    // Log startup
    if cli.verbose {
        info!("NexiBot CLI v{}", env!("CARGO_PKG_VERSION"));
        info!("Connecting to: {}", cli.api_url);
    }

    // Execute command
    match run_command(cli.command, client).await {
        Ok(_) => Ok(()),
        Err(e) => {
            eprintln!("{} {}", "error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

async fn run_command(command: Commands, client: NexiBotClient) -> Result<(), CliError> {
    match command {
        Commands::Chat(args) => commands::chat::handle(args, &client).await?,
        Commands::Memory(args) => commands::memory::handle(args, &client).await?,
        Commands::Voice(args) => commands::voice::handle(args, &client).await?,
        Commands::Session(args) => commands::session::handle(args, &client).await?,
        Commands::Skills(args) => commands::skills::handle(args, &client).await?,
        Commands::Agent(args) => commands::agent::handle(args, &client).await?,
        Commands::Config(args) => commands::config::handle(args, &client).await?,
        Commands::Auth(args) => commands::auth::handle(args, &client).await?,
        Commands::Batch(args) => commands::batch::handle(args, &client).await?,
        Commands::Security(args) => commands::security::handle(args, &client).await?,
        Commands::Status(args) => commands::status::handle(args, &client).await?,
        Commands::Completion(args) => commands::completion::handle(args).await?,
        Commands::Help2(args) => commands::help::handle(args)?,
    }
    Ok(())
}
