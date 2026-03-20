//! Help command - comprehensive command reference

use crate::error::CliError;
use crate::output;
use clap::Parser;

#[derive(Parser)]
pub struct HelpArgs {
    /// Show detailed help
    #[arg(short, long)]
    detailed: bool,
}

pub fn handle(args: HelpArgs) -> Result<(), CliError> {
    if args.detailed {
        print_detailed_help();
    } else {
        print_quick_help();
    }
    Ok(())
}

fn print_quick_help() {
    println!();
    output::info("NexiBot CLI - Command Reference");
    println!();
    println!("USAGE: nexibot <COMMAND> [OPTIONS]");
    println!();
    println!("COMMANDS:");
    println!("  chat          Send a message to Claude");
    println!("  memory        Search and manage memories");
    println!("  voice         Control voice system");
    println!("  session       Manage conversation sessions");
    println!("  skills        Discover and execute skills");
    println!("  agent         Control agent state");
    println!("  config        Manage configuration");
    println!("  auth          Manage API tokens");
    println!("  batch         Batch operations and scripting");
    println!("  status        Server health and status");
    println!("  completion    Shell completion generation");
    println!("  help          Show this help message");
    println!();
    println!("GLOBAL OPTIONS:");
    println!("  --api-url <URL>      NexiBot API server URL (default: http://localhost:18791)");
    println!("  --token <TOKEN>      API authentication token");
    println!("  --format <FORMAT>    Output format: json, yaml, table, plain (default: table)");
    println!("  -v, --verbose        Enable verbose output");
    println!("  --config <PATH>      Config file path");
    println!();
    println!("EXAMPLES:");
    println!("  nexibot chat \"What is the weather?\"");
    println!("  nexibot memory search \"my preferences\"");
    println!("  nexibot voice stop-listening");
    println!("  nexibot session list --format json");
    println!("  nexibot skills list");
    println!("  nexibot agent status");
    println!();
}

fn print_detailed_help() {
    print_quick_help();

    println!();
    output::info("DETAILED COMMAND REFERENCE");
    println!();

    println!("chat <MESSAGE>");
    println!("  Send a message to Claude and get a response.");
    println!("  Options:");
    println!("    --stream              Enable streaming mode");
    println!("    --with-memory         Include memory context");
    println!("    --with-skills         Include available skills");
    println!("    --thinking-budget N   Set thinking budget (if supported)");
    println!();

    println!("memory <SUBCOMMAND>");
    println!("  Search, list, and manage memories.");
    println!("  Subcommands:");
    println!("    search <QUERY>      Search memories (limit: --limit N)");
    println!("    list               List all memories (filter: --type-filter TYPE)");
    println!("    get <ID>           Get specific memory by ID");
    println!("    add <CONTENT>      Add new memory (--type, --tags)");
    println!("    delete <ID>        Delete a memory");
    println!();

    println!("voice <SUBCOMMAND>");
    println!("  Control voice system.");
    println!("  Subcommands:");
    println!("    listen             Start listening for wake word");
    println!("    stop-listening     Stop listening and return to idle");
    println!("    test-tts <TEXT>    Play test audio");
    println!("    status             Get voice status");
    println!("    toggle             Toggle voice response (TTS)");
    println!();

    println!("session <SUBCOMMAND>");
    println!("  Manage conversation sessions.");
    println!("  Subcommands:");
    println!("    list               List all sessions");
    println!("    new [NAME]         Create new session");
    println!("    load <ID>          Load a session");
    println!("    delete <ID>        Delete a session");
    println!("    info <ID>          Get session details");
    println!();

    println!("For more help: nexibot <COMMAND> --help");
    println!();
}
