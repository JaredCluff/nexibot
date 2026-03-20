# NexiBot

Your local AI agent. Private. Powerful. Yours.

NexiBot is a desktop AI agent that runs on your machine. It connects to the LLM providers you choose (local Ollama, Anthropic, OpenAI, Google), responds to voice commands, integrates with your messaging channels, and searches your local files semantically. All ML inference runs on-device via ONNX Runtime. No cloud required for core functionality.

## Features

- **Multi-provider LLM** with 4-level fallback chain (Ollama, Anthropic Claude, OpenAI, Google Gemini)
- **Voice assistant** with "Hey Nexus" wake word detection (OpenWakeWord ONNX)
- **8+ messaging channels**: Telegram, Discord, Slack, WhatsApp, Signal, Teams, Matrix, Email
- **Local semantic search**: LanceDB vectors + SQLite FTS5 with hybrid MMR re-ranking
- **On-device ML**: DeBERTa v3 prompt injection detection (<10ms), Silero VAD, SenseVoice STT, all-MiniLM-L6-v2 embeddings
- **Agent orchestration** with TF-IDF capability matching and subagent spawning
- **MCP server** (Model Context Protocol) for AI client integration
- **Browser automation** via Chrome DevTools Protocol
- **Computer Use API** (screenshot, mouse, keyboard) with confirmation gates
- **Skills system** with hot-reload, security scanning, and ClawHub marketplace
- **Session memory** with SQLite FTS5 full-text search and encrypted transcripts
- **K2K federation protocol** for knowledge routing across nodes
- **Cross-platform**: macOS, Windows, Linux

## Quick Start

### Download

Grab the latest release from the [Releases](https://github.com/jaredcluff/nexibot/releases) page:

- **macOS**: `.dmg` (Apple Silicon or Intel)
- **Windows**: `.msi` installer
- **Linux**: `.deb` or `.AppImage`

### Build from Source

Requires: Rust 1.75+, Node.js 22+, npm

```bash
git clone https://github.com/jaredcluff/nexibot.git
cd nexibot

# Install dependencies
cd ui && npm install && cd ..
cd anthropic-bridge && npm install && cd ..

# Build desktop app
cargo tauri build
```

See [detailed setup guide](docs/SETUP_NEXIBOT.md) for platform-specific instructions.

## Configuration

Config lives at standard XDG paths:

- **macOS**: `~/Library/Application Support/ai.nexibot.desktop/config.yaml`
- **Linux**: `~/.config/nexibot/config.yaml`
- **Windows**: `AppData\Local\nexibot\config.yaml`

Override with environment variables:

```bash
NEXIBOT_MODEL=claude-sonnet-4-5-20250929  # Override model
NEXIBOT_API_KEY=sk-...                     # Override API key
NEXIBOT_PROFILE=minimal                    # Load config.minimal.yaml overlay
RUST_LOG=nexibot_tauri=debug               # Debug logging
```

## Architecture

```
nexibot/
├── src-tauri/              # Tauri desktop app (Rust backend)
├── cli/                    # CLI commands
├── k2k-common/             # K2K federation protocol library
├── ui/                     # React frontend
├── anthropic-bridge/       # Node.js OAuth bridge
└── docs/                   # User documentation
```

**NexiBot Desktop** (Tauri): Full desktop app with React UI. Voice, messaging channels, agent orchestration, browser automation, and everything else.

**k2k-common**: Shared library implementing the K2K federation protocol. WebSocket transport, RSA-256 JWT auth, DNS-SD discovery.

**Anthropic Bridge**: Node.js service that bridges NexiBot to the Anthropic TypeScript SDK for OAuth token support. Runs locally on port 18790.

## Security

NexiBot runs a defense-in-depth security pipeline:

- DeBERTa v3 prompt injection detection (ONNX, <10ms)
- Llama Guard 3 content safety classification
- SSRF protection with fail-closed DNS resolution
- AES-256-GCM session encryption
- OS-native credential storage (Keychain, Credential Manager, Secret Service)
- 17-check security audit system

See [SECURITY.md](SECURITY.md) for vulnerability reporting.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

Apache 2.0. See [LICENSE](LICENSE).
