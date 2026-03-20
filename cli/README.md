# NexiBot Comprehensive CLI

A powerful command-line interface for controlling NexiBot from your terminal.

## Features

- **Chat**: Send messages to Claude directly from the command line
- **Memory Management**: Search and manage conversation memories with hybrid search
- **Voice Control**: Control voice system (wake word, listening, TTS)
- **Session Management**: Create, load, and manage conversation sessions
- **Skills Execution**: Discover and execute skills
- **Agent Control**: Pause, resume, or stop the agent
- **Configuration Management**: View and update settings
- **Batch Operations**: Run automated sequences from JSONL files
- **Multiple Output Formats**: JSON, YAML, table, or plain text
- **Scripting Support**: Full JSON output for integration with scripts and automation
- **Shell Completion**: Generate completion scripts for bash, zsh, and fish

## Installation

### Build from Source

```bash
cd cli
cargo build --release
# Binary at: target/release/nexibot
```

### Install Globally

```bash
cargo install --path cli
# Installs to ~/.cargo/bin/nexibot
```

## Quick Start

```bash
# Check if NexiBot server is running
nexibot status

# Send a message
nexibot chat "What is the weather?"

# Search memories
nexibot memory search "my preferences"

# List sessions
nexibot session list

# Get agent status
nexibot agent status
```

## Configuration

### Environment Variables

```bash
# API server URL (default: http://localhost:18791)
export NEXIBOT_API_URL="http://localhost:18791"

# API authentication token
export NEXIBOT_API_TOKEN="your-token-here"

# Default output format (json, yaml, table, plain)
export NEXIBOT_FORMAT="json"
```

### Configuration File

Create `~/.config/nexibot/cli.toml`:

```toml
# API server configuration
api_url = "http://localhost:18791"
token = "your-token-here"

# Default output format
default_format = "table"

# Request timeout in seconds
timeout = 30

# Command aliases
[aliases]
hello = "chat 'Say hello!'"
list-all = "memory list"
```

## Command Reference

### Chat

Send messages to Claude:

```bash
# Simple message
nexibot chat "What is 2+2?"

# With memory context
nexibot chat "Remember my preferences" --with-memory

# With available skills
nexibot chat "What skills do I have?" --with-skills

# Set thinking budget (for extended thinking models)
nexibot chat "Complex question" --thinking-budget 5000

# JSON output
nexibot chat "Hello" --format json
```

### Memory

Manage memories:

```bash
# Search memories
nexibot memory search "my preferences"
nexibot memory search "project" --limit 5

# List all memories
nexibot memory list
nexibot memory list --type-filter preference

# Get specific memory
nexibot memory get <memory-id>

# Add a new memory
nexibot memory add "I prefer dark mode" --type preference --tags ui theme

# Delete a memory
nexibot memory delete <memory-id>
```

### Voice

Control voice system:

```bash
# Start listening for wake word
nexibot voice listen

# Stop listening
nexibot voice stop-listening

# Get voice status
nexibot voice status

# Test text-to-speech
nexibot voice test-tts "Hello world"

# Toggle voice response (TTS)
nexibot voice toggle
```

### Session

Manage conversation sessions:

```bash
# List all sessions
nexibot session list

# Create new session
nexibot session new
nexibot session new "Project Planning"

# Load a session
nexibot session load <session-id>

# Get session info
nexibot session info <session-id>

# Delete a session
nexibot session delete <session-id>
```

### Skills

Discover and execute skills:

```bash
# List available skills
nexibot skills list

# Get skill info
nexibot skills info search_web

# Execute a skill
nexibot skills exec --name search_web --args query="climate change"
```

### Agent

Control agent state:

```bash
# Get agent status
nexibot agent status

# Resume agent (if paused)
nexibot agent resume

# Pause agent (queue messages)
nexibot agent pause

# Emergency stop
nexibot agent stop
```

### Config

Manage configuration:

```bash
# Get all config
nexibot config get

# Get specific setting
nexibot config get claude.model

# Set configuration
nexibot config set claude.model claude-opus-4-6

# Reset to defaults
nexibot config reset
```

### Auth

Manage API tokens:

```bash
# Login with token
nexibot auth login your-api-token-here

# Show current token (masked for security)
nexibot auth show

# Logout
nexibot auth logout
```

### Batch

Batch operations and scripting:

```bash
# Run batch operations from JSONL file
nexibot batch run operations.jsonl
nexibot batch run operations.jsonl --stop-on-error

# Schedule operations with cron expression
nexibot batch schedule "0 9 * * *" 'nexibot chat "Good morning!"'
```

### Status

Server health check:

```bash
# Basic status
nexibot status

# Detailed status
nexibot status --detailed
```

### Help

Get comprehensive help:

```bash
# Quick help
nexibot help

# Detailed help with examples
nexibot help --detailed

# Command-specific help
nexibot chat --help
nexibot memory search --help
```

## Output Formats

### JSON

Perfect for scripting and automation:

```bash
nexibot chat "Hello" --format json
# Output:
# {
#   "response": "Hello! How can I help?",
#   "model": "claude-opus-4-6"
# }
```

### YAML

Human-readable structured format:

```bash
nexibot session list --format yaml
```

### Table

Default format with pretty formatting:

```bash
nexibot session list --format table
# ┌──────────────────┬──────────────┬───────────────┐
# │ ID               │ Title        │ Messages      │
# ├──────────────────┼──────────────┼───────────────┤
# │ sess_abc123      │ Project      │ 45            │
# └──────────────────┴──────────────┴───────────────┘
```

### Plain

Simple key=value format:

```bash
nexibot chat "Hello" --format plain
# response=Hello! How can I help?
# model=claude-opus-4-6
```

## Scripting Examples

### Shell Script

```bash
#!/bin/bash

# Search memories and log results
nexibot memory search "project status" --format json | \
  jq '.results | length'

# Send daily summary
TIME=$(date +%H:%M)
nexibot chat "Summarize today's conversations" --format json | \
  jq -r '.response' | mail -s "Daily Summary" user@example.com
```

### Batch Operations File (JSONL)

Create `batch_ops.jsonl`:

```jsonl
{"command": "chat", "message": "What is my status?"}
{"command": "memory", "action": "search", "query": "recent projects"}
{"command": "session", "action": "list"}
```

Run with:

```bash
nexibot batch run batch_ops.jsonl
```

### Cron Integration

Add to crontab:

```cron
# Run daily status check at 9 AM
0 9 * * * /usr/local/bin/nexibot chat "Daily status report" --format json >> /tmp/nexibot.log

# Search memories every hour
0 * * * * /usr/local/bin/nexibot memory search "urgent" --format json | \
  jq -r '.results[0].content' | \
  /usr/bin/mail -s "Urgent Memory" admin@example.com
```

### Python Integration

```python
import subprocess
import json

def call_nexibot(cmd):
    """Execute nexibot command and return JSON result"""
    result = subprocess.run(
        f"nexibot {cmd} --format json",
        shell=True,
        capture_output=True,
        text=True
    )
    return json.loads(result.stdout)

# Search memories
memories = call_nexibot("memory search 'project'")
for memory in memories['results']:
    print(f"- {memory['content']}")

# Send message
response = call_nexibot("chat 'Summarize today'")
print(response['response'])
```

## Advanced Usage

### Multiple Servers

```bash
# Server 1
nexibot --api-url http://localhost:18791 chat "Message 1"

# Server 2
nexibot --api-url http://remote-server:18791 chat "Message 2"
```

### Verbose Logging

```bash
# Show detailed operation logs
nexibot -v chat "Hello"
nexibot --verbose memory search "query"
```

### Piping Commands

```bash
# Pipe memory content to another command
nexibot memory search "code" --format plain | \
  grep -i python | \
  wc -l

# Format conversion
nexibot session list --format json | \
  jq '.sessions[] | .title'
```

### Integration with Other Tools

```bash
# With fzf for fuzzy selection
nexibot session list --format plain | \
  fzf | \
  awk '{print $1}' | \
  xargs -I {} nexibot session load {}

# With grep for filtering
nexibot memory list --format plain | \
  grep "project"

# With sort and uniq for analysis
nexibot memory list --format plain | \
  cut -d= -f2 | \
  sort | uniq -c
```

## Troubleshooting

### "Server unreachable" Error

```bash
# Check if NexiBot is running
nexibot status

# Verify API URL
nexibot --api-url http://your-server:port status

# Check network connectivity
curl http://localhost:18791/api/health
```

### Authentication Failed

```bash
# Verify token is set
echo $NEXIBOT_API_TOKEN

# Check if token is valid
nexibot --token your-token auth show

# Update token in config
nexibot auth login new-token-here
```

### No Output

```bash
# Enable verbose mode to see what's happening
nexibot -v chat "test"

# Try different output format
nexibot chat "test" --format json
```

### Performance Issues

```bash
# Reduce verbosity
nexibot --api-url http://localhost:18791 chat "fast query"

# Use plain format (faster rendering)
nexibot memory list --format plain
```

## Shell Completion

### Bash

```bash
# Add to ~/.bashrc
eval "$(nexibot completion bash)"
```

### Zsh

```bash
# Add to ~/.zshrc
eval "$(nexibot completion zsh)"
```

### Fish

```bash
# Add to ~/.config/fish/config.fish
nexibot completion fish | source
```

## API Endpoints Reference

The CLI communicates with these HTTP endpoints:

- `POST /api/chat/send` - Send a message
- `GET /api/config` - Get configuration
- `PUT /api/config` - Update configuration
- `GET /api/sessions` - List sessions
- `GET /api/models` - Get available models
- `GET /api/skills` - List skills
- `GET /api/health` - Health check

## Architecture

```
┌─────────────────────────────────────┐
│  nexibot CLI Binary                 │
│  - Clap: Argument parsing           │
│  - Reqwest: HTTP client             │
│  - Serde: JSON/YAML serialization   │
└─────────────┬───────────────────────┘
              │ HTTP
              ▼
┌─────────────────────────────────────┐
│  NexiBot API Server (Axum)          │
│  - /api/chat/send                   │
│  - /api/memory/*                    │
│  - /api/voice/*                     │
│  - /api/session/*                   │
│  - /api/config                      │
└─────────────────────────────────────┘
```

## Development

### Building

```bash
cargo build          # Debug build
cargo build --release  # Optimized release build
```

### Testing

```bash
cargo test
cargo test -- --test-threads=1  # Single-threaded testing
```

### Code Style

```bash
cargo fmt          # Format code
cargo clippy       # Lint code
```

## License

Apache-2.0 - See LICENSE

## Contributing

For bug reports, feature requests, or contributions, please open an issue on GitHub.
